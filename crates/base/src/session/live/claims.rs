//! Agent presence on a live text doc (amendment 13): claims and their waiters, the claim
//! policy on edits, heads-up notes, and the activity ring. A child of `live`, so it works
//! on the session's own state. Everything here is memory only (see `session::awareness`).

use super::*;
use crate::session::awareness::{
    Claim, ClaimEnd, ClaimId, ClaimInfo, ForEdit, Note, NoteKind, Reservation, ReservationEnd,
    ReservationInfo, Waiting,
};
use crate::session::awareness::{
    Overwrite, OverwriteCell, OVERWRITE_CELLS_MAX, OVERWRITE_PENDING_MAX,
};

impl Session {
    // ----- claims: the public calls -----

    /// Claim `range` (lines, 1-based, inclusive; `last` past the end is clamped) for
    /// `ttl_ms` ([`Config::claim_default_ms`] when `None`, at most
    /// [`Config::claim_max_ms`]; `Some(0)` is refused). Needs edit. Advisory: it refuses
    /// nobody's edit unless the host asks for that on the edit ([`ClaimPolicy::Respect`]),
    /// and never a person's.
    ///
    /// The range is anchored (expanding): it follows concurrent edits, and text typed at
    /// its edges lands inside. It ends:
    /// - by default, when the claimant's own edit touching it lands; with `hold`, only on
    ///   release, time or leave;
    /// - when its time runs out ([`Session::expire_idle`] / the manager's `tick`, and lazily
    ///   on any claim, roster, read or edit call);
    /// - on [`Session::release_claim`], or when the connection that took it leaves (or is
    ///   kicked, idles out, loses read).
    ///
    /// Each end is an [`AuditEvent::ClaimEnded`] with the waiters in order.
    ///
    /// Errors: another participant's live claim overlapping it gives
    /// [`SessionError::Claimed`] (exit 5, with who, their lines, time left and note); the
    /// caller's own overlapping claim that others wait for gives
    /// [`SessionError::ClaimContended`] (no getting around the queue by re-claiming); a bad
    /// range [`SessionError::BadPosition`]; limits ([`Config::claims_max`],
    /// [`Config::claims_per_participant`]) [`SessionError::BadClaim`].
    pub fn claim(
        &self,
        conn: ConnId,
        range: LineRange,
        ttl_ms: Option<u64>,
        note: Option<String>,
        hold: bool,
    ) -> Result<ClaimInfo> {
        let out = if self.is_grid() {
            self.claim_span_inner(
                conn,
                crate::session::ClaimSpan::Lines(range),
                ttl_ms,
                note,
                hold,
            )
        } else {
            self.claim_inner(conn, range, ttl_ms, note, hold)
        };
        self.for_viewer(conn, out, |l, c| l.claim(c))
    }

    pub(super) fn claim_inner(
        &self,
        conn: ConnId,
        range: LineRange,
        ttl_ms: Option<u64>,
        note: Option<String>,
        hold: bool,
    ) -> Result<ClaimInfo> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Edit, Action::Claim, now)?;
        self.expire_claims(st, now);
        let ttl = ttl_ms.unwrap_or(self.config.claim_default_ms);
        self.take_claim(
            st,
            i,
            &range.root,
            (range.first, range.last),
            ttl,
            note,
            hold,
            false,
            "claim",
            now,
        )
    }

    /// Extend a claim of the caller's pid to `now + ttl_ms` (as [`Session::claim`]), and set
    /// or clear `hold`. `id: None` is the caller's only claim in this doc. Needs edit.
    ///
    /// **Anti-starvation:** while another participant waits for the claim
    /// ([`Session::register_waiter`]), it can't be extended (a later expiry) or given
    /// `hold`: [`SessionError::ClaimContended`] (exit 2), and nothing changes. A shorter
    /// TTL or clearing `hold` is allowed. The claim still ends normally.
    ///
    /// Errors: [`SessionError::NoClaim`] (exit 4); someone else's claim, or several claims
    /// and no id: [`SessionError::BadClaim`] (exit 2).
    pub fn renew_claim(
        &self,
        conn: ConnId,
        id: Option<ClaimId>,
        ttl_ms: Option<u64>,
        hold: Option<bool>,
    ) -> Result<ClaimInfo> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Edit, Action::Claim, now)?;
        self.expire_claims(st, now);
        let k = Self::own_claim(st, i, id)?;
        let ttl = self.claim_ttl(ttl_ms.unwrap_or(self.config.claim_default_ms))?;
        let c = &st.aware.claims[k];
        let expires = now.saturating_add(ttl);
        let extends = expires > c.expires_ms || (hold == Some(true) && !c.hold);
        if extends && c.contended() {
            return Err(SessionError::ClaimContended {
                id: c.id,
                waiters: c.waiters.len(),
                remaining_ms: c.remaining(now),
            });
        }
        let c = &mut st.aware.claims[k];
        c.expires_ms = expires;
        if let Some(h) = hold {
            c.hold = h;
        }
        let id = c.id;
        let info = self.claim_info(st, k, now)?;
        self.log_activity(
            st,
            i,
            ActivityKind::Claim,
            "claim --renew",
            Some(info.lines.clone()),
            now,
            Some(id),
        );
        Ok(info)
    }

    /// Release the caller's claim `id`, or (`None`) every claim of the caller's pid in this
    /// doc. Returns the claims released (empty when none was live and the caller's: the
    /// `claim --release` verb exits 1). Needs read.
    pub fn release_claim(&self, conn: ConnId, id: Option<ClaimId>) -> Result<Vec<ClaimId>> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Read, Action::Claim, now)?;
        self.expire_claims(st, now);
        let pid = st.members[i].p.pid.clone();
        let mut out = Vec::new();
        while let Some(k) = st
            .aware
            .claims
            .iter()
            .position(|c| c.pid == pid && id.map_or(true, |x| x == c.id))
        {
            out.push(st.aware.claims[k].id);
            self.end_claim(st, k, ClaimEnd::Released, now, true);
        }
        Ok(out)
    }

    /// Every live claim in the doc, oldest first, as it is now. Needs read.
    pub fn claims(&self, conn: ConnId) -> Result<Vec<ClaimInfo>> {
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
        self.expire_claims(st, now);
        (0..st.aware.claims.len())
            .map(|k| self.claim_info(st, k, now))
            .collect()
    }

    /// The time left on claim `id`, on the host's clock now (deterministic: `expires_ms -
    /// now_ms`). Needs read. [`SessionError::NoClaim`] when it isn't live.
    pub fn claim_remaining_ms(&self, conn: ConnId, id: ClaimId) -> Result<u64> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        self.admit(st, conn, Need::Read, Action::Claim, now)?;
        self.expire_claims(st, now);
        st.aware
            .claims
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.remaining(now))
            .ok_or(SessionError::NoClaim(Some(id)))
    }

    /// Wait for another participant's claim `id`: the connection joins its queue (FIFO;
    /// again is a no-op that reports the place). Needs edit. **The kit never sleeps or
    /// blocks:** the host waits for [`AuditEvent::ClaimEnded`] (its `next_waiter` and
    /// `waiters`), with its own timeout and [`Session::cancel_waiter`]. While anyone waits,
    /// the owner can't renew or hold the claim. A waiting connection doesn't idle out; it
    /// leaves the queue when it leaves the doc.
    ///
    /// Errors: [`SessionError::NoClaim`]; the caller's own pid's claim, or a full queue
    /// ([`Config::claim_waiters_max`]): [`SessionError::BadClaim`].
    pub fn register_waiter(&self, conn: ConnId, id: ClaimId) -> Result<Waiting> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Edit, Action::Claim, now)?;
        self.expire_claims(st, now);
        let pid = st.members[i].p.pid.clone();
        let k = st
            .aware
            .claims
            .iter()
            .position(|c| c.id == id)
            .ok_or(SessionError::NoClaim(Some(id)))?;
        let c = &mut st.aware.claims[k];
        if c.pid == pid {
            return Err(SessionError::BadClaim(format!(
                "{id} is your own claim; release it instead of waiting"
            )));
        }
        let position = match c.waiters.iter().position(|(w, _)| *w == conn) {
            Some(p) => p + 1,
            None => {
                if c.waiters.len() >= self.config.claim_waiters_max {
                    return Err(SessionError::BadClaim(format!(
                        "{} already waiting for {id}",
                        c.waiters.len()
                    )));
                }
                c.waiters.push_back((conn, pid));
                c.waiters.len()
            }
        };
        Ok(Waiting {
            claim: self.claim_info(st, k, now)?,
            position,
        })
    }

    /// Leave claim `id`'s queue. Returns whether the connection was waiting. Needs read.
    pub fn cancel_waiter(&self, conn: ConnId, id: ClaimId) -> Result<bool> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        self.admit(st, conn, Need::Read, Action::Claim, now)?;
        self.expire_claims(st, now);
        // the reservation handed to this connection: it passes to the next waiter
        if let Some(k) = st
            .aware
            .reservations
            .iter()
            .position(|r| r.claim == id && r.conn == conn)
        {
            self.end_reservation(st, k, ReservationEnd::Cancelled, now);
            return Ok(true);
        }
        let mut found = false;
        for r in st.aware.reservations.iter_mut().filter(|r| r.claim == id) {
            let before = r.queue.len();
            r.queue.retain(|(w, _)| *w != conn);
            found |= r.queue.len() != before;
        }
        if let Some(c) = st.aware.claims.iter_mut().find(|c| c.id == id) {
            let before = c.waiters.len();
            c.waiters.retain(|(w, _)| *w != conn);
            found |= c.waiters.len() != before;
        }
        Ok(found)
    }

    /// The live hand-off reservations (see [`Config::claim_handoff_ms`]), oldest first.
    /// Needs read.
    pub fn reservations(&self, conn: ConnId) -> Result<Vec<ReservationInfo>> {
        let out = self.reservations_inner(conn);
        self.for_viewer(conn, out, |l, v| {
            v.into_iter().map(|r| l.reservation(r)).collect()
        })
    }

    fn reservations_inner(&self, conn: ConnId) -> Result<Vec<ReservationInfo>> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        self.admit(st, conn, Need::Read, Action::Claim, now)?;
        self.expire_claims(st, now);
        (0..st.aware.reservations.len())
            .map(|k| self.any_reservation_info(st, k, now))
            .collect()
    }

    /// The last `n` rows of the doc's activity ring, oldest first: reads (through
    /// `run_tool`), changes, claims taken and ended. Needs read (the same as the roster).
    /// The ring holds [`Config::activity_max`] rows, in memory only; it is dropped when the
    /// doc unloads, and a participant erasure drops that pid's rows.
    pub fn activity(&self, conn: ConnId, n: usize) -> Result<Vec<ActivityEntry>> {
        let out = self.activity_inner(conn, n);
        self.for_viewer(conn, out, |l, v| {
            v.into_iter().map(|a| l.activity(a)).collect()
        })
    }

    fn activity_inner(&self, conn: ConnId, n: usize) -> Result<Vec<ActivityEntry>> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        self.admit(st, conn, Need::Read, Action::Activity, now)?;
        self.expire_claims(st, now);
        let a = &st.aware.activity;
        Ok(a.iter().skip(a.len().saturating_sub(n)).cloned().collect())
    }

    // ----- claims: internal -----

    pub(super) fn claim_ttl(&self, ttl: u64) -> Result<u64> {
        if ttl == 0 {
            return Err(SessionError::BadClaim(
                "a claim needs a TTL above 0 ms".into(),
            ));
        }
        Ok(ttl.min(self.config.claim_max_ms.max(1)))
    }

    fn claim_info(&self, st: &State, k: usize, now: u64) -> Result<ClaimInfo> {
        self.any_claim_info(st, k, now)
    }

    /// The caller's pid's claim `id` (or its only one).
    fn own_claim(st: &State, i: usize, id: Option<ClaimId>) -> Result<usize> {
        let pid = &st.members[i].p.pid;
        match id {
            Some(id) => {
                let k = st
                    .aware
                    .claims
                    .iter()
                    .position(|c| c.id == id)
                    .ok_or(SessionError::NoClaim(Some(id)))?;
                if st.aware.claims[k].pid != *pid {
                    return Err(SessionError::BadClaim(format!(
                        "{id} is {}'s claim, not yours",
                        st.aware.claims[k].pid
                    )));
                }
                Ok(k)
            }
            None => {
                let mine: Vec<usize> = (0..st.aware.claims.len())
                    .filter(|k| st.aware.claims[*k].pid == *pid)
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

    /// Take a claim on lines `first..=last` of `root` for member `i` (rights already
    /// checked, expired claims already ended).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn take_claim(
        &self,
        st: &mut State,
        i: usize,
        root: &str,
        (first, last): (usize, usize),
        ttl: u64,
        note: Option<String>,
        hold: bool,
        for_edit: bool,
        verb: &str,
        now: u64,
    ) -> Result<ClaimInfo> {
        let ttl = self.claim_ttl(ttl)?;
        let text = self.text_of(st, root)?.1;
        let Some((s, e, _)) = awareness::line_span(text, first, last) else {
            return Err(SessionError::BadPosition(format!(
                "lines {first}..{last} of a {}-line text",
                position::line_count(text)
            )));
        };
        let pid = st.members[i].p.pid.clone();
        // a reservation handed to someone else binds agents only
        if st.members[i].p.kind == ParticipantKind::Agent {
            if let Some(r) = st
                .aware
                .reservations
                .iter()
                .find(|r| r.pid != pid && r.overlaps(root, s, e))
            {
                return Err(SessionError::Claimed {
                    id: r.claim,
                    by: r.pid.clone(),
                    name: r.name.clone(),
                    lines: line_pair(text, r.s, r.e),
                    remaining_ms: r.remaining(now),
                    note: None,
                });
            }
        }
        for c in st.aware.claims.iter().filter(|c| c.overlaps(root, s, e)) {
            if c.pid != pid {
                return Err(SessionError::Claimed {
                    id: c.id,
                    by: c.pid.clone(),
                    name: c.name.clone(),
                    lines: line_pair(text, c.s, c.e),
                    remaining_ms: c.remaining(now),
                    note: c.note.clone(),
                });
            }
            if c.contended() {
                return Err(SessionError::ClaimContended {
                    id: c.id,
                    waiters: c.waiters.len(),
                    remaining_ms: c.remaining(now),
                });
            }
        }
        let c = &self.config;
        if st.aware.claims.len() >= c.claims_max {
            return Err(SessionError::BadClaim(format!(
                "this doc has {} live claims (the most)",
                st.aware.claims.len()
            )));
        }
        let mine = st.aware.claims.iter().filter(|c| c.pid == pid).count();
        if mine >= c.claims_per_participant {
            return Err(SessionError::BadClaim(format!(
                "you hold {mine} claims here (the most); release one"
            )));
        }
        // the caller's own reservation there is used: the rest of its queue waits for this
        let mut waiters = VecDeque::new();
        if let Some(k) = st
            .aware
            .reservations
            .iter()
            .position(|r| r.pid == pid && r.overlaps(root, s, e))
        {
            let r = st.aware.reservations.remove(k);
            waiters = r.queue;
            waiters.retain(|(_, p)| *p != pid);
            self.emit(AuditEvent::ReservationEnded {
                key: st.key.clone(),
                claim: r.claim,
                pid: r.pid,
                how: ReservationEnd::Claimed,
                lines: self.lines_now(st, &r.root, r.s, r.e),
                version: st.version,
                next_waiter: waiters.front().cloned(),
            });
        }
        let text = self.text_of(st, root)?.1;
        let m = &st.members[i];
        let claim = Claim {
            id: ClaimId::fresh(),
            pid: pid.clone(),
            name: m.p.name.clone(),
            conn: m.conn,
            root: root.to_owned(),
            s,
            e,
            taken_ms: now,
            expires_ms: now.saturating_add(ttl),
            note,
            hold,
            for_edit,
            waiters,
            edited: false,
            grid: None,
        };
        let info = claim.info(text, now);
        st.aware.claims.push(claim);
        st.roster.record(
            RosterEvent::ClaimStarted {
                who: Who::of(&pid),
                claim: info.id,
                lines: info.lines.clone(),
                note: info.note.clone(),
                hold,
                for_edit,
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
            for_edit,
        });
        self.log_activity(
            st,
            i,
            ActivityKind::Claim,
            verb,
            Some(info.lines.clone()),
            now,
            Some(info.id),
        );
        Ok(info)
    }

    /// End claim `k`: remove it, tell the host (with its queue), log it; with `grant`, hand
    /// its range (a grid claim: its rows, columns or cells) to the first waiter still here
    /// (a reservation).
    pub(super) fn end_claim(&self, st: &mut State, k: usize, how: ClaimEnd, now: u64, grant: bool) {
        let c = st.aware.claims.remove(k);
        let lines = if c.grid.is_some() {
            self.grid_claim_lines(st, &c)
        } else {
            self.lines_now(st, &c.root, c.s, c.e)
        };
        let waiters: Vec<(ConnId, String)> = c.waiters.into_iter().collect();
        st.roster.record(
            RosterEvent::ClaimEnded {
                who: Who::of(&c.pid),
                claim: c.id,
                how,
                lines: lines.clone(),
                version: st.version,
            },
            self.config.roster_queue,
        );
        self.emit(AuditEvent::ClaimEnded {
            key: st.key.clone(),
            claim: c.id,
            pid: c.pid.clone(),
            how,
            lines: lines.clone(),
            version: st.version,
            next_waiter: waiters.first().cloned(),
            waiters: waiters.clone(),
        });
        if grant {
            let span = (c.s, c.e);
            self.grant_reservation(st, c.id, &c.root, span, c.grid.clone(), waiters.into(), now);
        }
        st.aware.push_activity(
            ActivityEntry {
                pid: c.pid,
                name: c.name,
                kind: ActivityKind::ClaimEnded(how),
                verb: format!("claim {}", how.as_str()),
                lines: Some(lines),
                at_ms: now,
                claim: Some(c.id),
            },
            self.config.activity_max,
        );
    }

    /// The lines of `root`'s anchored span `(s, e)` in the live text now (the events'
    /// `lines` as a claim or reservation ends). A schema's roots always exist; a missing one
    /// would give line 1.
    fn lines_now(&self, st: &State, root: &str, s: usize, e: usize) -> LineRange {
        let text = self.text_of(st, root).map_or("", |(_, t)| t);
        awareness::span_lines(root, text, s, e)
    }

    /// Reserve `root`'s span `(s, e)` (claim `claim`'s, now ended; on a grid, its span by
    /// key, `grid`) for the first waiter in `queue` still present, for
    /// [`Config::claim_handoff_ms`]. Off when that is 0.
    #[allow(clippy::too_many_arguments)]
    fn grant_reservation(
        &self,
        st: &mut State,
        claim: ClaimId,
        root: &str,
        (s, e): (usize, usize),
        grid: Option<awareness::GridSpan>,
        mut queue: VecDeque<(ConnId, String)>,
        now: u64,
    ) {
        let ms = self.config.claim_handoff_ms;
        if ms == 0 {
            return;
        }
        while let Some((conn, pid)) = queue.pop_front() {
            let Some(m) = st.members.iter().find(|m| m.conn == conn) else {
                continue;
            };
            let r = Reservation {
                claim,
                pid,
                name: m.p.name.clone(),
                conn,
                root: root.to_owned(),
                s,
                e,
                until_ms: now.saturating_add(ms),
                queue,
                edited: false,
                grid,
            };
            let lines = if r.grid.is_some() {
                self.grid_reservation_lines(st, &r)
            } else {
                match self.text_of(st, root) {
                    Ok((_, t)) => awareness::span_lines(root, t, s, e),
                    Err(_) => return,
                }
            };
            self.emit(AuditEvent::Reserved {
                key: st.key.clone(),
                claim,
                pid: r.pid.clone(),
                conn,
                lines,
                until_ms: r.until_ms,
                waiters: r.queue.iter().cloned().collect(),
            });
            st.aware.reservations.push(r);
            return;
        }
    }

    /// End reservation `k` (`how`); unless it was used by a claim or reset, pass it to the
    /// next waiter.
    pub(super) fn end_reservation(&self, st: &mut State, k: usize, how: ReservationEnd, now: u64) {
        let r = st.aware.reservations.remove(k);
        let lines = if r.grid.is_some() {
            self.grid_reservation_lines(st, &r)
        } else {
            self.lines_now(st, &r.root, r.s, r.e)
        };
        self.emit(AuditEvent::ReservationEnded {
            key: st.key.clone(),
            claim: r.claim,
            pid: r.pid,
            how,
            lines,
            version: st.version,
            next_waiter: r.queue.front().cloned(),
        });
        let passes = !matches!(
            how,
            ReservationEnd::Claimed | ReservationEnd::Reset | ReservationEnd::Deleted
        );
        if passes {
            self.grant_reservation(st, r.claim, &r.root, (r.s, r.e), r.grid, r.queue, now);
        }
    }

    /// End every claim whose time is up, and lapse every reservation whose deadline passed.
    pub(super) fn expire_claims(&self, st: &mut State, now: u64) {
        while let Some(k) = st.aware.claims.iter().position(|c| c.expires_ms <= now) {
            self.end_claim(st, k, ClaimEnd::Expired, now, true);
        }
        while let Some(k) = st.aware.reservations.iter().position(|r| r.until_ms <= now) {
            self.end_reservation(st, k, ReservationEnd::Lapsed, now);
        }
    }

    /// End the claims a just-committed change by their owner touched, and the
    /// reservations it used (passing them on).
    pub(super) fn end_edited_claims(&self, st: &mut State, now: u64) {
        while let Some(k) = st.aware.claims.iter().position(|c| c.edited) {
            self.end_claim(st, k, ClaimEnd::Edited, now, true);
        }
        while let Some(k) = st.aware.reservations.iter().position(|r| r.edited) {
            self.end_reservation(st, k, ReservationEnd::Edited, now);
        }
    }

    /// End every claim (`how`) and reservation, handing nothing on: a reload after a
    /// failed write, or the session closing.
    pub(super) fn end_all_claims(&self, st: &mut State, how: ClaimEnd) {
        let now = self.host.now_ms();
        while !st.aware.claims.is_empty() {
            self.end_claim(st, 0, how, now, false);
        }
        while !st.aware.reservations.is_empty() {
            self.end_reservation(st, 0, ReservationEnd::Reset, now);
        }
    }

    /// A connection left: its claims end (handed to their waiters), it leaves every queue,
    /// and a reservation held for it passes on.
    pub(super) fn conn_gone(&self, st: &mut State, conn: ConnId) {
        let now = self.host.now_ms();
        for c in &mut st.aware.claims {
            c.waiters.retain(|(w, _)| *w != conn);
        }
        for r in &mut st.aware.reservations {
            r.queue.retain(|(w, _)| *w != conn);
        }
        while let Some(k) = st.aware.reservations.iter().position(|r| r.conn == conn) {
            self.end_reservation(st, k, ReservationEnd::Left, now);
        }
        while let Some(k) = st.aware.claims.iter().position(|c| c.conn == conn) {
            self.end_claim(st, k, ClaimEnd::Left, now, true);
        }
    }

    /// The connection holds, waits for, or has been handed a live claim's range.
    pub(super) fn in_claims(st: &State, conn: ConnId) -> bool {
        st.aware
            .claims
            .iter()
            .any(|c| c.conn == conn || c.waiters.iter().any(|(w, _)| *w == conn))
            || st
                .aware
                .reservations
                .iter()
                .any(|r| r.conn == conn || r.queue.iter().any(|(w, _)| *w == conn))
    }

    /// The claim policy on an edit by member `i` whose `hunks` (live-text coordinates)
    /// change `root`: under `Respect`, an **agent's** edit that touches another
    /// participant's live claim, or a reservation handed to someone else, is refused.
    /// Claims coordinate agents; anyone else (a person, an app guest) is never refused.
    pub(super) fn check_claims(
        &self,
        st: &mut State,
        i: usize,
        root: &str,
        hunks: &[Hunk],
        policy: ClaimPolicy,
        now: u64,
    ) -> Result<()> {
        self.expire_claims(st, now);
        if policy == ClaimPolicy::Ignore || st.members[i].p.kind != ParticipantKind::Agent {
            return Ok(());
        }
        let pid = &st.members[i].p.pid;
        let text = self.text_of(st, root)?.1;
        let fates = awareness::claim_fates(&st.aware.claims, root, hunks, pid);
        if let Some((c, _)) = st
            .aware
            .claims
            .iter()
            .zip(&fates)
            .find(|(c, f)| c.pid != *pid && f.touched)
        {
            return Err(SessionError::Claimed {
                id: c.id,
                by: c.pid.clone(),
                name: c.name.clone(),
                lines: line_pair(text, c.s, c.e),
                remaining_ms: c.remaining(now),
                note: c.note.clone(),
            });
        }
        if let Some(r) = st
            .aware
            .reservations
            .iter()
            .find(|r| r.pid != *pid && r.touched_by(root, hunks))
        {
            return Err(SessionError::Claimed {
                id: r.claim,
                by: r.pid.clone(),
                name: r.name.clone(),
                lines: line_pair(text, r.s, r.e),
                remaining_ms: r.remaining(now),
                note: None,
            });
        }
        Ok(())
    }

    /// A read verb's `--for-edit`: an implicit claim on the lines shown, unless another
    /// participant's claim overlaps them (then nothing is taken and that claim is
    /// reported).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn for_edit_claim(
        &self,
        st: &mut State,
        i: usize,
        root: &str,
        lines: Option<(usize, usize)>,
        ttl_ms: Option<u64>,
        verb: &str,
        now: u64,
    ) -> ForEdit {
        let Some(lines) = lines else {
            return ForEdit::NotTaken(SessionError::BadClaim(
                "the call showed no lines to claim".into(),
            ));
        };
        if !st.members[i].rights.edit {
            return ForEdit::NotTaken(SessionError::Forbidden { need: Need::Edit });
        }
        let ttl = ttl_ms.unwrap_or(self.config.for_edit_ms);
        match self.take_claim(st, i, root, lines, ttl, None, false, true, verb, now) {
            Ok(c) => ForEdit::Claimed(c),
            Err(e @ SessionError::Claimed { .. }) => {
                let SessionError::Claimed { id, .. } = &e else {
                    unreachable!()
                };
                match st.aware.claims.iter().position(|c| c.id == *id) {
                    Some(k) => match self.claim_info(st, k, now) {
                        Ok(c) => ForEdit::Conflict(c),
                        Err(e) => ForEdit::NotTaken(e),
                    },
                    // a reservation handed to someone else
                    None => ForEdit::NotTaken(e),
                }
            }
            Err(e) => ForEdit::NotTaken(e),
        }
    }

    // ----- notes -----

    /// Every heads-up about `[s, e]` of `root` for `reader`, raw (no dedupe, no limit),
    /// most important first. `edit_window: None` names editors whatever the age (conflict
    /// reports).
    pub(super) fn collect_notes(
        &self,
        st: &State,
        reader: &str,
        root: &str,
        (s, e): (usize, usize),
        edit_window: Option<u64>,
        now: u64,
    ) -> Vec<Note> {
        let Ok((_, text)) = self.text_of(st, root) else {
            return Vec::new();
        };
        let kind_of = |pid: &str| {
            st.members
                .iter()
                .find(|m| m.p.pid == pid)
                .map_or(ParticipantKind::Agent, |m| m.p.kind)
        };
        let note = |pid: &str, name: &str, kind: NoteKind, (a, b): (usize, usize), at: u64| Note {
            pid: pid.to_owned(),
            name: name.to_owned(),
            kind,
            participant_kind: kind_of(pid),
            lines: awareness::span_lines(root, text, a, b),
            age_ms: now.saturating_sub(at),
            claim: None,
            claim_note: None,
            remaining_ms: None,
            span: None,
            overwrite: None,
        };
        let mut out: Vec<Note> = Vec::new();
        for c in &st.aware.claims {
            if c.pid != reader && c.overlaps(root, s, e) {
                out.push(Note {
                    claim: Some(c.id),
                    claim_note: c.note.clone(),
                    remaining_ms: Some(c.remaining(now)),
                    ..note(&c.pid, &c.name, NoteKind::Claimed, (c.s, c.e), c.taken_ms)
                });
            }
        }
        for m in &st.members {
            if m.p.pid == reader {
                continue;
            }
            if let Some((r, a, b)) = &m.view {
                if r == root && awareness::overlap((*a, *b), (s, e)) {
                    let at = m.view_ms.unwrap_or(m.last_active_ms);
                    out.push(note(&m.p.pid, &m.p.name, NoteKind::Viewing, (*a, *b), at));
                }
            }
            if let (Some((r, a, b)), Some(at)) = (&m.edited, m.last_edit_ms) {
                let recent = edit_window.map_or(true, |w| now.saturating_sub(at) < w);
                if r == root && recent && awareness::overlap((*a, *b), (s, e)) {
                    if let Some(lines) = awareness::edit_span_lines(root, text, *a, *b) {
                        let mut n = note(&m.p.pid, &m.p.name, NoteKind::Edited, (*a, *b), at);
                        n.lines = lines;
                        out.push(n);
                    }
                }
            }
            if m.p.kind == ParticipantKind::Person {
                if let Some(c) = m.cursor.as_ref().filter(|c| c.root == root) {
                    let (a, b) = (c.anchor.min(c.head), c.anchor.max(c.head));
                    if awareness::overlap((a, b), (s, e)) {
                        let kind = if a == b {
                            NoteKind::Caret
                        } else {
                            NoteKind::Selection
                        };
                        let at = m.last_cursor_ms.unwrap_or(m.last_active_ms);
                        out.push(note(&m.p.pid, &m.p.name, kind, (a, b), at));
                    }
                }
            }
        }
        out.sort_by(|x, y| (x.kind, x.age_ms, &x.pid).cmp(&(y.kind, y.age_ms, &y.pid)));
        // one note per (other, kind, lines), however many connections
        let mut seen: Vec<(String, NoteKind, usize, usize)> = Vec::new();
        out.retain(|n| {
            let k = (n.pid.clone(), n.kind, n.lines.first, n.lines.last);
            if seen.contains(&k) {
                false
            } else {
                seen.push(k);
                true
            }
        });
        out
    }

    /// The heads-up notes for member `i` touching lines `first..=last` of `root`: deduped
    /// per (reader, other, kind, lines) within [`Config::note_repeat_ms`], at most one
    /// batch per reader per [`Config::note_interval_ms`], at most
    /// [`Config::note_batch_max`] per batch.
    ///
    /// Notes waiting for this reader (an outside save may have removed their text, or, on a
    /// grid, overrode their value) always come first, whatever the lines, the interval or the
    /// batch size: any verb that returns notes delivers them.
    pub(super) fn notes_for(
        &self,
        st: &mut State,
        i: usize,
        root: &str,
        lines: Option<(usize, usize)>,
        now: u64,
    ) -> Notes {
        debug_assert!(
            !self.is_grid(),
            "grid notes are delivered by grid_notes_for_edits / batch_notes"
        );
        let waiting = self.drain_notes(st, i, now);
        let mut out = self.fresh_notes(st, i, root, lines, now);
        if !waiting.is_empty() {
            let waiting = match Self::app_viewer(st, i) {
                Some(v) => {
                    let mut l = st.handles.lens(&v);
                    waiting.into_iter().map(|n| l.note(n)).collect()
                }
                None => waiting,
            };
            let mut notes: Vec<Note> = waiting;
            notes.append(&mut out.notes);
            out.notes = notes;
        }
        out
    }

    /// [`Session::notes_for`] without the waiting notes: what a host's own edit call gets,
    /// which leaves them where they are (taken, they'd be lost).
    pub(super) fn fresh_notes(
        &self,
        st: &mut State,
        i: usize,
        root: &str,
        lines: Option<(usize, usize)>,
        now: u64,
    ) -> Notes {
        let Some((first, last)) = lines else {
            return Notes::default();
        };
        let Some((s, e, _)) = self
            .text_of(st, root)
            .ok()
            .and_then(|(_, t)| awareness::line_span(t, first, last))
        else {
            return Notes::default();
        };
        let reader = st.members[i].p.pid.clone();
        let window = self.config.note_edit_window_ms;
        let raw = self.collect_notes(st, &reader, root, (s, e), Some(window), now);
        let mut out = self.batch_fresh(st, &reader, raw, now);
        out.notes = Self::notes_as_seen(st, i, out.notes);
        out
    }

    /// Raw notes for `reader`, deduped per (reader, other, kind, lines) within
    /// [`Config::note_repeat_ms`], at most one batch per reader per
    /// [`Config::note_interval_ms`], at most [`Config::note_batch_max`] per batch.
    ///
    /// For member `i`; an App reader (K5) sees the others by handle.
    pub(super) fn batch_notes(&self, st: &mut State, i: usize, raw: Vec<Note>, now: u64) -> Notes {
        let reader = st.members[i].p.pid.clone();
        // notes waiting for this reader (a change overrode its value) always come
        let waiting = self.drain_notes(st, i, now);
        let mut out = self.batch_fresh(st, &reader, raw, now);
        if !waiting.is_empty() {
            let mut notes = waiting;
            notes.append(&mut out.notes);
            out.notes = notes;
        }
        out.notes = Self::notes_as_seen(st, i, out.notes);
        out
    }

    /// `notes` as member `i` sees them: an App viewer (K5) by handle, anyone else unchanged.
    fn notes_as_seen(st: &mut State, i: usize, notes: Vec<Note>) -> Vec<Note> {
        match Self::app_viewer(st, i) {
            Some(v) => {
                let mut l = st.handles.lens(&v);
                notes.into_iter().map(|n| l.note(n)).collect()
            }
            None => notes,
        }
    }

    /// The store's waiting notes into the mirror, once; the expired ones go (a note lives as
    /// long as the revert window: past it, the change is gone from the revert records too).
    pub(super) fn load_notes(&self, st: &mut State, now: u64) -> Result<()> {
        if st.aware.notes_loaded || !self.structured {
            return Ok(());
        }
        st.aware.notes_loaded = true;
        let store = self.store(st)?.clone();
        for (pid, b) in store.overwrite_notes(self.doc_id)? {
            let mut at = 0;
            let mut q = Vec::new();
            while at < b.len() {
                let Some(o) = Overwrite::decode(&b, &mut at) else {
                    break;
                };
                if now.saturating_sub(o.at_ms) <= self.config.revert_keep_ms {
                    q.push(Self::overwritten_note(o));
                }
            }
            if q.is_empty() {
                store.set_overwrite_notes(self.doc_id, &[(pid, Vec::new())])?;
            } else {
                st.aware.pending.insert(pid, q);
            }
        }
        Ok(())
    }

    /// The note for one [`Overwrite`].
    pub(super) fn overwritten_note(o: Overwrite) -> Note {
        let line = o.line.unwrap_or(1);
        Note {
            pid: crate::session::EXTERNAL_PID.to_owned(),
            name: crate::session::EXTERNAL_PID.to_owned(),
            kind: NoteKind::Overwritten,
            participant_kind: ParticipantKind::Person,
            lines: LineRange {
                root: crate::table::ROWS.to_owned(),
                first: line,
                last: line,
            },
            age_ms: 0,
            claim: None,
            claim_note: None,
            remaining_ms: None,
            span: None,
            overwrite: Some(o),
        }
    }

    /// `notes` encoded for the store.
    pub(super) fn encode_notes(notes: &[Note]) -> Vec<u8> {
        let mut w = Vec::new();
        for o in notes.iter().filter_map(|n| n.overwrite.as_ref()) {
            o.encode(&mut w);
        }
        w
    }

    /// A grid doc's notes waiting for `pid`, delivered: gone from the mirror and the store, each
    /// as `rights` may have it ([`Session::grid_note_for_reader`]; [`Session::drain_notes`] for
    /// a grid).
    pub(super) fn drain_grid_notes(
        &self,
        st: &mut State,
        pid: &str,
        rights: &Rights,
        now: u64,
    ) -> Vec<Note> {
        if self.load_notes(st, now).is_err() {
            return Vec::new();
        }
        let Some(mut q) = st.aware.pending.remove(pid) else {
            return Vec::new();
        };
        if let Ok(store) = self.store(st) {
            let _ = store.set_overwrite_notes(self.doc_id, &[(pid.to_owned(), Vec::new())]);
            for o in q.iter_mut().filter_map(|n| n.overwrite.as_mut()) {
                self.grid_note_for_reader(store, o, rights);
            }
        }
        q
    }

    /// A grid note as `rights` may have it, as a text doc's ([`Session::note_for_reader`]): the
    /// version that holds its live value only for a reader with the history right, and never
    /// for a withheld note (its value was purged); no `restore_from` without history.
    fn grid_note_for_reader(
        &self,
        store: &crate::store::DocStore,
        o: &mut Overwrite,
        rights: &Rights,
    ) {
        o.version = if rights.history && !o.withheld {
            o.version_of(self.version_before(store, o.before))
        } else {
            None
        };
        if !rights.history {
            o.restore_from = None;
        }
        o.no_history = !rights.history;
    }

    /// A grid doc's waiting [`Overwrite`] notes for a snapshot, each as its reader may have it.
    /// Someone who can't read the doc now has none here ([`Session::waiting_for_subscribers`]
    /// for a grid).
    pub(super) fn grid_waiting_for_subscribers(
        &self,
        st: &State,
    ) -> HashMap<String, Vec<Overwrite>> {
        let Ok(store) = self.store(st) else {
            return HashMap::new();
        };
        st.aware
            .pending
            .iter()
            .filter_map(|(pid, q)| {
                let rights = st
                    .members
                    .iter()
                    .find(|m| m.p.pid == *pid)
                    .map(|m| self.host.rights(&m.p, &st.key))
                    .filter(|r| r.read)?;
                let list = q
                    .iter()
                    .filter_map(|n| n.overwrite.clone())
                    .map(|mut o| {
                        self.grid_note_for_reader(store, &mut o, &rights);
                        o
                    })
                    .collect();
                Some((pid.clone(), list))
            })
            .collect()
    }

    /// The newest version at or before change `before`: it holds the live value a change after
    /// it overrode.
    pub(super) fn version_before(
        &self,
        store: &crate::store::DocStore,
        before: Seq,
    ) -> Option<crate::store::VersionId> {
        store
            .version_infos(self.doc_id)
            .ok()?
            .into_iter()
            .filter(|v| v.seq <= before)
            .max_by_key(|v| (v.seq, v.id))
            .map(|v| v.id)
    }

    /// Queue a grid note `o` for `pid`'s `q`: on its row's note if it has one for this change
    /// (one note per row per author), at most [`OVERWRITE_PENDING_MAX`] notes per pid, the
    /// overflow rolled up in the last one ("N more cells overwritten"): never dropped silently.
    /// (A text doc's notes queue in `live/removed.rs`.)
    pub(super) fn queue_grid_overwrite(q: &mut Vec<Note>, o: Overwrite) {
        if o.rule == "cell" && o.row.is_some() {
            let same = q
                .iter_mut()
                .rev()
                .filter_map(|n| n.overwrite.as_mut())
                .find(|x| x.rule == "cell" && x.row == o.row && x.change == o.change);
            if let Some(x) = same {
                if x.also.len() + 1 < OVERWRITE_CELLS_MAX {
                    x.also.push(OverwriteCell {
                        region: o.region,
                        col: o.col,
                        yours: o.yours,
                        file: o.file,
                        cut: o.cut,
                    });
                } else {
                    x.more += 1;
                }
                return;
            }
        }
        if q.len() < OVERWRITE_PENDING_MAX {
            q.push(Self::overwritten_note(o));
            return;
        }
        let (cells, change, before, at_ms) = (o.cells(), o.change, o.before, o.at_ms);
        let last = q.last_mut().and_then(|n| n.overwrite.as_mut());
        match last {
            Some(r) if r.is_rollup() => {
                r.more += cells;
                r.change = change;
                r.before = r.before.max(before);
                r.at_ms = at_ms;
            }
            _ => {
                // the last note becomes part of the roll-up too, to keep the bound
                let old = q.pop().and_then(|n| n.overwrite);
                let more = cells + old.as_ref().map_or(0, Overwrite::cells);
                let mut r = o;
                r.rule = "rollup";
                r.region.clear();
                r.row = None;
                r.line = None;
                r.col = None;
                r.yours.clear();
                r.file.clear();
                r.cut = false;
                r.also.clear();
                r.more = more;
                q.push(Self::overwritten_note(r));
            }
        }
    }

    fn batch_fresh(&self, st: &mut State, reader: &str, raw: Vec<Note>, now: u64) -> Notes {
        let c = &self.config;
        st.aware
            .prune_noted(now, c.note_repeat_ms, c.note_interval_ms);
        let key = |n: &Note| {
            (
                reader.to_owned(),
                n.pid.clone(),
                n.kind,
                n.lines.root.clone(),
                n.lines.first,
                n.lines.last,
            )
        };
        let fresh: Vec<Note> = raw
            .into_iter()
            .filter(|n| !st.aware.noted.contains_key(&key(n)))
            .collect();
        if fresh.is_empty() {
            return Notes::default();
        }
        if st.aware.batches.contains_key(reader) {
            return Notes {
                held: fresh.len(),
                ..Notes::default()
            };
        }
        let more = fresh.len().saturating_sub(c.note_batch_max);
        let shown: Vec<Note> = fresh.into_iter().take(c.note_batch_max).collect();
        if shown.is_empty() {
            return Notes {
                more,
                ..Notes::default()
            };
        }
        for n in &shown {
            st.aware.noted.insert(key(n), now);
            st.roster.record(
                RosterEvent::Note {
                    to: Who::of(reader),
                    from: Who::of(&n.pid),
                    kind: n.kind,
                    lines: n.lines.clone(),
                    claim: n.claim,
                },
                self.config.roster_queue,
            );
        }
        st.aware.batches.insert(reader.to_owned(), now);
        Notes {
            notes: shown,
            more,
            held: 0,
        }
    }

    // ----- activity -----

    /// A row in the ring for member `i`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn log_activity(
        &self,
        st: &mut State,
        i: usize,
        kind: ActivityKind,
        verb: &str,
        lines: Option<LineRange>,
        now: u64,
        claim: Option<ClaimId>,
    ) {
        let m = &st.members[i];
        let e = ActivityEntry {
            pid: m.p.pid.clone(),
            name: m.p.name.clone(),
            kind,
            verb: verb.to_owned(),
            lines,
            at_ms: now,
            claim,
        };
        st.aware.push_activity(e, self.config.activity_max);
    }
}

/// The lines a byte span covers now, as `(first, last)`.
fn line_pair(text: &str, s: usize, e: usize) -> (usize, usize) {
    (position::line_of(text, s), position::line_of(text, e))
}
