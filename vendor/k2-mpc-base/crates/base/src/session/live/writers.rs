//! Person-writer reuse ([`Config::reuse_person_writers`], v0.14.0): one writer per person per device, so a doc's
//! writer count follows people and devices, not connections.
//!
//! A person's (or App guest's) join with a device key ([`JoinOptions::device`]) takes a free
//! writer of that person on that device, or a fresh one, under a **lease** (a random token per
//! hand-out). A writer is
//! - **held** while a present connection's lease is on it (never by two: selection skips held
//!   writers, and a hand-over removes the holder before the new connection is pushed);
//! - **free** after a release ([`Session::release`]: the client holds nothing unsent and is
//!   done with the writer), or after any end of a connection without client state
//!   ([`JoinOptions::device_stateless`]);
//! - **parked** after every other end (a dropped socket, idle, a kick, lost rights, a close,
//!   an unload, a crash): only a reclaim naming that lease ([`JoinOptions::reclaim`]) takes it
//!   back, unless [`Config::person_writer_park_ms`] lets it go after that long.
//!
//! **Why reuse can't fork a writer's line.** Every op under a member's writer is made by the
//! session's own doc, under the lock, at that writer's next clock in the doc (Yrs:
//! `transact_mut_as`; native: `next_seq`), and leaves the session only after its change is
//! durable (`commit`: the store before any broadcast; `recover` reloads on a failure). No
//! client sends ops: a returning client sends hunks at a base, applied as whatever writer its
//! new connection holds. What reuse must keep safe is the client's bookkeeping keyed on its
//! writer (its lost-change watch, its minted tagged keys) and a reconnect's close of its old
//! connection: hence the lease token, `kept`, and `prev_ended`.
//!
//! **Rows.** Each writer's row is `client_kinds` kind 2 ([`PersonWriterRow`]), with its owner
//! row. Rows ride in transactions the session opens anyway: the rows changed since are
//! written with the next change's commit ([`DocStore::commit_changes_with`]) and at the
//! session's close; a join or a leave opens none. A lease that never landed a change and was
//! lost to a crash leaves its writer as the row had it (free or parked): nothing of it is in
//! the doc. At load, a row still leased whose end wasn't recorded (a crash) ends at the loaded
//! version: every change of that lease is in the store by then.
//!
//! **The writers cap** ([`Config::writers_max`]). The doc's writers are the IDs its state holds
//! or may come to hold: every ID with an owner row, every person writer whose row is written
//! (a connection that could write has held it), and every present connection counted when it
//! could write. A connection that could write, joining or gaining edit rights, needs room for
//! a writer the count doesn't hold yet. At the cap a person's join takes, in order: a free
//! writer of that person and device (reuse mints nothing); that person's free writer of
//! another device (re-tagged to this one); that person's oldest parked writer, let go early
//! as the park expiry lets it go (its reclaim keeps nothing and is told when its lease ended).
//! Otherwise, and for any other join that would need a new writer, the connection reads only
//! (`Joined::writers_full`) and every edit is refused `MPC-E908`. The outside participant, the
//! import and a tier-0 promotion's authors are never refused, and count.

use std::collections::BTreeMap;

use super::*;
use crate::session::WRITERS_NOTICE_MAX;
use crate::store::PersonWriterRow;

/// How a member's lease ends when the member leaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum End {
    /// Any end but these two: parked (free for a stateless connection).
    Unclean,
    /// [`Session::release`]: free.
    Release,
    /// A reclaim moved the writer to the connection joining now: the lease goes on.
    Handover,
}

/// A member's person-writer lease.
#[derive(Debug, Clone, Copy)]
pub(super) struct MemberLease {
    pub(super) token: u64,
    pub(super) device: [u8; 16],
    pub(super) stateless: bool,
    pub(super) end: End,
}

/// One person-device writer the session knows.
#[derive(Debug, Clone)]
struct Writer {
    owner: String,
    row: PersonWriterRow,
    /// The row as the store holds it (`None`: never written).
    stored: Option<PersonWriterRow>,
    /// A connection that could write has held it (or the store has its row): only then is its
    /// row written. Every row stays in the store for good, so a viewer's writer, which never
    /// writes, leaves none; it gets one when its connection gains edit rights.
    writable: bool,
}

/// The person-device writers of the session's doc, by client (lowest first).
#[derive(Debug, Default)]
pub(super) struct Writers {
    map: BTreeMap<ClientId, Writer>,
}

impl Writers {
    /// The doc's rows, with a crash's unrecorded lease ends recorded at `version`.
    pub(super) fn load(store: &DocStore, id: DocId, version: Seq, now: u64) -> Result<Self> {
        let mut map = BTreeMap::new();
        for (client, owner, stored) in store.person_writers(id)? {
            let mut row = stored;
            if row.leased && row.ended_token != row.token && !planted(Plant::NoLoadEnd) {
                // its lease's end wasn't recorded (a crash): no change of it lands after this
                // load's version, every one of them is in the store
                row.ended_token = row.token;
                row.ended_seq = version;
                row.ended_ms = now;
            }
            map.insert(
                client,
                Writer {
                    owner,
                    row,
                    stored: Some(stored),
                    writable: true,
                },
            );
        }
        Ok(Self { map })
    }

    pub(super) fn contains(&self, client: ClientId) -> bool {
        self.map.contains_key(&client)
    }

    /// The rows that differ from the store's, to write (only writers a connection that could
    /// write has held).
    pub(super) fn dirty(&self) -> Vec<(ClientId, String, PersonWriterRow)> {
        self.map
            .iter()
            .filter(|(_, w)| w.writable && w.stored != Some(w.row))
            .map(|(c, w)| (*c, w.owner.clone(), w.row))
            .collect()
    }

    /// A connection holding `client` can write now: its row is written from now on.
    pub(super) fn writable(&mut self, client: ClientId) {
        if let Some(w) = self.map.get_mut(&client) {
            w.writable = true;
        }
    }

    /// `rows` landed in the store.
    pub(super) fn stored(&mut self, rows: &[(ClientId, String, PersonWriterRow)]) {
        for (c, _, row) in rows {
            if let Some(w) = self.map.get_mut(c) {
                w.stored = Some(*row);
            }
        }
    }
}

/// A person-writer join's inputs ([`JoinOptions::device`], [`JoinOptions::reclaim`]).
pub(super) struct PersonJoin<'a> {
    pub(super) key: &'a [u8],
    pub(super) stateless: bool,
    pub(super) reclaim: Option<WriterLease>,
}

impl<'a> PersonJoin<'a> {
    /// The person-writer part of `opts`, when reuse is on and the key fits.
    pub(super) fn of(config: &Config, opts: &'a JoinOptions) -> Option<Self> {
        let key = opts.device.as_ref()?;
        if !config.reuse_person_writers || key.0.is_empty() || key.0.len() > DEVICE_KEY_MAX {
            return None;
        }
        Some(Self {
            key: &key.0,
            stateless: opts.stateless,
            reclaim: opts.reclaim,
        })
    }
}

/// Whether the doc has room for one more writer ([`Session::writer_room`]).
#[derive(Debug, Clone, Copy)]
pub(super) struct Room {
    /// The doc holds `writers_max` writers or more: only a writer it counts already is taken.
    pub(super) full: bool,
}

impl Room {
    /// Room, without counting (no connection that could write).
    pub(super) const OPEN: Room = Room { full: false };
}

/// The writer a person-writer join takes.
pub(super) struct Pick {
    pub(super) client: ClientId,
    /// The reclaimed lease's own writer.
    pub(super) kept: bool,
    /// The present member holding it, to close first (a hand-over).
    pub(super) from: Option<usize>,
    pub(super) prev_ended: Option<Seq>,
    /// The doc is at its writers cap and `client` isn't a writer it counts: the connection
    /// reads only.
    pub(super) full: bool,
}

/// Whether `p` takes part in person-writer reuse: a person or an App guest editing through
/// the session alone (never an own replica: its replica may still write as its writer; agents
/// keep their own reuse, [`Config::reuse_agent_clients`]).
pub(super) fn takes_part(p: &Participant) -> bool {
    !p.own_replica && matches!(p.kind, ParticipantKind::Person | ParticipantKind::App)
}

/// A device key's tag: HMAC-SHA256 under the doc's hash key, first 16 bytes. The store never
/// holds the host's key; a purge rotates the hash key, so tags from before it match nothing.
pub(super) fn device_tag(hash_key: &[u8; 32], key: &[u8]) -> [u8; 16] {
    let h = crate::hash::hmac(hash_key, key);
    let mut t = [0u8; 16];
    t.copy_from_slice(&h[..16]);
    t
}

/// A lease token: random, never 0. No security boundary (the host scopes a reclaim to the
/// authenticated person); it tells a connection's own lease from a later one.
pub(super) fn new_token() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    loop {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        h.write_u128(nanos);
        let t = h.finish();
        if t != 0 {
            return t;
        }
    }
}

impl Session {
    /// The writer a join of `p` on device `tag` takes ([`Config::reuse_person_writers`]):
    /// the reclaimed lease's own (held by this person's connection with that lease: a
    /// hand-over; or parked with that lease), else the lowest free writer of this person and
    /// device (a parked one whose park expired too), else a fresh one. At the writers cap
    /// (`room.full`), a writer the doc counts: that person's free writer of another device, else
    /// that person's oldest parked writer; else the connection reads only (`Pick::full`).
    pub(super) fn pick_person_writer(
        &self,
        st: &State,
        p: &Participant,
        tag: [u8; 16],
        reclaim: Option<WriterLease>,
        now: u64,
        room: Room,
    ) -> Result<Pick> {
        let held = |c: ClientId| st.members.iter().any(|m| m.client == c);
        let mut prev_ended = None;
        if let Some(l) = reclaim {
            let holder = st.members.iter().position(|m| {
                m.client == l.client
                    && m.p.pid == p.pid
                    && m.lease
                        .is_some_and(|ml| ml.token == l.token && ml.device == tag)
            });
            if let Some(j) = holder.filter(|_| !planted(Plant::ReclaimNeverKeeps)) {
                return Ok(Pick {
                    client: l.client,
                    kept: true,
                    from: Some(j),
                    prev_ended: None,
                    full: room.full && !counted(st, l.client),
                });
            }
            if let Some(w) = st.writers.map.get(&l.client) {
                let own = w.owner == p.pid;
                if own
                    && w.row.device == tag
                    && w.row.leased
                    && !w.row.retired
                    && w.row.token == l.token
                    && !held(l.client)
                    && !planted(Plant::ReclaimNeverKeeps)
                {
                    return Ok(Pick {
                        client: l.client,
                        kept: true,
                        from: None,
                        prev_ended: None,
                        full: room.full && !counted(st, l.client),
                    });
                }
                if own
                    && l.token != 0
                    && w.row.ended_token == l.token
                    && !planted(Plant::ExpiryWithoutCap)
                {
                    prev_ended = Some(w.row.ended_seq);
                }
            }
        }
        let park = self.config.person_writer_park_ms;
        // a reclaim that didn't take its writer back never gets that writer here either: its
        // client still watches its old changes under it, and would read a later lease's
        // changes under it as its own
        let stale = reclaim
            .map(|l| l.client)
            .filter(|_| !planted(Plant::StaleReclaimTakesOwn));
        let parked_out = |w: &Writer| {
            w.row.ended_token == w.row.token
                && park > 0
                && !planted(Plant::ExpiryIgnored)
                && now.saturating_sub(w.row.ended_ms) >= park
        };
        let free = st
            .writers
            .map
            .iter()
            .find(|(c, w)| {
                let takeable = if held(**c) {
                    // held: never (a planted fault takes it anyway)
                    planted(Plant::ReusePersonHeld)
                } else {
                    !w.row.leased || parked_out(w) || planted(Plant::ReuseParked)
                };
                w.owner == p.pid
                    && w.row.device == tag
                    && !w.row.retired
                    && takeable
                    && Some(**c) != stale
            })
            .map(|(c, _)| *c);
        // at the cap, only a writer the doc counts already
        let fits = |c: ClientId| !room.full || counted(st, c);
        if let Some(c) = free.filter(|c| fits(*c)) {
            return Ok(Pick {
                client: c,
                kept: false,
                from: None,
                prev_ended,
                full: false,
            });
        }
        if room.full {
            // the person's writers the doc counts, none held, none the stale reclaim's own
            let mine = || {
                st.writers.map.iter().filter(|(c, w)| {
                    w.owner == p.pid
                        && !w.row.retired
                        && !held(**c)
                        && Some(**c) != stale
                        && counted(st, **c)
                })
            };
            // a free writer of another device (re-tagged to this one by its lease)
            let other = mine()
                .find(|(_, w)| !w.row.leased || parked_out(w))
                .map(|(c, _)| *c);
            // else the oldest parked writer, let go early: as the park expiry does, so its
            // reclaim keeps nothing and is told when its lease ended
            let parked =
                || mine().filter(|(_, w)| w.row.leased && w.row.ended_token == w.row.token);
            let oldest = if planted(Plant::CapTakesNewestParked) {
                parked().max_by_key(|(c, w)| (w.row.ended_ms, **c))
            } else {
                parked().min_by_key(|(c, w)| (w.row.ended_ms, **c))
            };
            if let Some(c) = other.or(oldest.map(|(c, _)| *c)) {
                return Ok(Pick {
                    client: c,
                    kept: false,
                    from: None,
                    prev_ended,
                    full: false,
                });
            }
        }
        // a free writer the doc doesn't count, or a new one: past the cap it reads only
        let client = match free {
            Some(c) => c,
            None => fresh_client(self.store(st)?, self.doc_id, |c| {
                held(c) || st.writers.contains(c)
            })?,
        };
        Ok(Pick {
            client,
            kept: false,
            from: None,
            prev_ended,
            full: room.full,
        })
    }

    /// Whether the doc has room for one more writer ([`Config::writers_max`]); counts nothing
    /// when there is no cap and the near-cap notice was given.
    pub(super) fn writer_room(&self, st: &mut State) -> Result<Room> {
        let max = self.config.writers_max;
        if max == 0 && st.writers_told {
            return Ok(Room::OPEN);
        }
        self.owners(st)?;
        let full = max > 0 && count_writers(st) >= max && !planted(Plant::CapUnchecked);
        Ok(Room { full })
    }

    /// `pid`'s connections that could not write so far and can now: each is counted when the
    /// doc has room for its writer (or counts it already), else it reads only from now on (its
    /// writer can't change mid-connection). A watcher is skipped until it is let in.
    pub(super) fn count_gains(&self, st: &mut State, pid: &str) -> Result<()> {
        let gaining: Vec<usize> = (0..st.members.len())
            .filter(|&k| {
                let m = &st.members[k];
                m.p.pid == pid && !m.counted && !m.writers_full && m.may_write()
            })
            .collect();
        if gaining.is_empty() {
            return Ok(());
        }
        for k in gaining {
            let room = self.writer_room(st)?;
            let c = st.members[k].client;
            if room.full && !counted(st, c) && !planted(Plant::CapJoinOnly) {
                st.members[k].writers_full = true;
            } else {
                st.members[k].counted = true;
            }
        }
        self.note_writers(st);
        Ok(())
    }

    /// The near-cap notice ([`AuditEvent::WritersNearCap`]), once per load: the doc holds 80%
    /// or more of `writers_max`, or of [`WRITERS_NOTICE_MAX`] when there is no cap.
    pub(super) fn note_writers(&self, st: &mut State) {
        if st.writers_told || st.owners.is_none() {
            return;
        }
        let max = match self.config.writers_max {
            0 if planted(Plant::NoNearCapNotice) => return,
            0 => WRITERS_NOTICE_MAX,
            m => m,
        };
        let count = count_writers(st);
        if count >= near_cap(max) {
            st.writers_told = true;
            self.emit(AuditEvent::WritersNearCap {
                key: st.key.clone(),
                count: count as u64,
                max: max as u64,
            });
        }
    }

    /// `MPC-E908` for this doc now.
    pub(super) fn writers_full_error(&self, st: &mut State) -> Result<SessionError> {
        self.owners(st)?;
        Ok(SessionError::WritersFull {
            count: count_writers(st) as u64,
            max: self.config.writers_max as u64,
        })
    }

    /// Lease `client` to a connection of `pid` on device `tag` with `token`; `can_write`: the
    /// connection has edit rights (else its row waits until it does).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lease_writer(
        &self,
        st: &mut State,
        client: ClientId,
        pid: &str,
        tag: [u8; 16],
        token: u64,
        can_write: bool,
    ) {
        let w = st.writers.map.entry(client).or_insert_with(|| Writer {
            owner: pid.to_owned(),
            row: PersonWriterRow::default(),
            stored: None,
            writable: false,
        });
        w.writable |= can_write;
        w.row.leased = true;
        w.row.token = token;
        w.row.device = tag;
        if let Some(o) = st.owners.as_mut() {
            o.entry(client).or_insert_with(|| pid.to_owned());
        }
    }

    /// A member holding `client` under `lease` left: its lease ends as `lease.end` says.
    pub(super) fn lease_ended(&self, st: &mut State, client: ClientId, lease: Option<MemberLease>) {
        let Some(l) = lease else {
            return;
        };
        let version = st.version;
        let Some(w) = st.writers.map.get_mut(&client) else {
            return; // a purge reloaded the rows since (its tags no longer match)
        };
        if !w.row.leased || w.row.token != l.token {
            return; // a later lease's now
        }
        match l.end {
            End::Handover => {}
            End::Release => {
                w.row.leased = false;
                w.row.token = 0;
            }
            End::Unclean if l.stateless => {
                w.row.leased = false;
                w.row.token = 0;
            }
            End::Unclean => {
                w.row.ended_token = l.token;
                w.row.ended_seq = version;
                w.row.ended_ms = self.host.now_ms();
            }
        }
    }

    /// The session closes: every lease ends (parked, or free for a stateless connection), and
    /// the rows changed since the last change landed are written, in one transaction. A
    /// failure is ignored: the rows only serve reuse, and a lost one leaves its writer as the
    /// store had it (free, or parked: never handed out held).
    pub(super) fn close_writers(&self, st: &mut State) {
        let ends: Vec<(ClientId, Option<MemberLease>)> =
            st.members.iter().map(|m| (m.client, m.lease)).collect();
        for (c, l) in ends {
            self.lease_ended(st, c, l);
        }
        let rows = st.writers.dirty();
        if rows.is_empty() {
            return;
        }
        if let Some(s) = st.store.as_ref() {
            if s.put_person_writers(self.doc_id, &rows).is_ok() {
                st.writers.stored(&rows);
            }
        }
    }

    /// A participant purge renamed owners and rotated the hash key: the rows are read again
    /// (an erased person's writers are never picked again; tags from before match nothing).
    pub(super) fn reload_writers(&self, st: &mut State) -> Result<()> {
        if !self.config.reuse_person_writers {
            return Ok(());
        }
        let now = self.host.now_ms();
        let w = Writers::load(self.store(st)?, self.doc_id, st.version, now)?;
        st.writers = w;
        Ok(())
    }

    /// Close one connection now, **cleanly** (person-writer reuse): its client holds nothing
    /// unsent (no edit, undo or redo in flight, an empty queue, no minted key it hasn't sent)
    /// and will never use this writer or its lease again (the wire's `bye`). Its writer is free
    /// for the person's next connection on that device. Otherwise as [`Session::leave`]:
    /// returns false when it wasn't present. A connection without a person-writer lease just
    /// leaves.
    pub fn release(&self, conn: ConnId) -> Result<bool> {
        self.now();
        let mut st = self.lock_for(conn)?;
        match Self::member(&st, conn) {
            Ok(i) => {
                if let Some(l) = st.members[i].lease.as_mut() {
                    l.end = End::Release;
                }
                self.remove_member(&mut st, i, LeaveReason::Left);
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }

    /// Test-only: every person-device writer the session knows, with its row and whether a
    /// present connection holds it.
    #[cfg(any(test, feature = "differential"))]
    #[doc(hidden)]
    pub fn person_writers_for_tests(
        &self,
    ) -> Result<Vec<(ClientId, String, PersonWriterRow, bool)>> {
        let st = self.lock()?;
        Ok(st
            .writers
            .map
            .iter()
            .map(|(c, w)| {
                let held = st.members.iter().any(|m| m.client == *c);
                (*c, w.owner.clone(), w.row, held)
            })
            .collect())
    }

    /// Test-only: each present connection with its writer.
    #[cfg(any(test, feature = "differential"))]
    #[doc(hidden)]
    pub fn member_clients_for_tests(&self) -> Result<Vec<(ConnId, ClientId)>> {
        let st = self.lock()?;
        Ok(st.members.iter().map(|m| (m.pres.conn, m.client)).collect())
    }
}

/// The writers the doc's count holds ([`Config::writers_max`]): the owner rows (built), the
/// person writers whose rows are written, and the counted present connections, each once.
fn count_writers(st: &State) -> usize {
    let owners = st.owners.as_ref().expect("built by the caller");
    let rows = st
        .writers
        .map
        .iter()
        .filter(|(c, w)| w.writable && !owners.contains_key(*c))
        .count();
    let present = st
        .members
        .iter()
        .filter(|m| {
            m.counted
                && !owners.contains_key(&m.client)
                && !st.writers.map.get(&m.client).is_some_and(|w| w.writable)
        })
        .count();
    owners.len() + rows + present
}

/// Whether the doc's count holds `c` (the owners built).
fn counted(st: &State, c: ClientId) -> bool {
    st.owners.as_ref().is_some_and(|o| o.contains_key(&c))
        || st.writers.map.get(&c).is_some_and(|w| w.writable)
        || st.members.iter().any(|m| m.client == c && m.counted)
}

/// 80% of `max`, rounded up: where the near-cap notice comes.
pub(super) fn near_cap(max: usize) -> usize {
    (max * 4).div_ceil(5)
}

/// No two present connections hold one writer (checked after every join; a planted fault
/// that breaks it is left for the rows to catch at their own assertions).
pub(super) fn one_holder_each(st: &State) -> bool {
    if any_planted() {
        return true;
    }
    let mut seen = std::collections::HashSet::new();
    st.members.iter().all(|m| seen.insert(m.client))
}

/// The planted person-writer faults (`crate::differential::ClientMutant`), by name. Without
/// the `differential` feature there are no hooks: none is planted.
#[derive(Clone, Copy)]
enum Plant {
    ReusePersonHeld,
    ReuseParked,
    HandoverWithoutClose,
    StaleReclaimTakesOwn,
    ExpiryWithoutCap,
    NoLoadEnd,
    ExpiryIgnored,
    ReclaimNeverKeeps,
    CapUnchecked,
    CapTakesNewestParked,
    CapOnExternal,
    CapJoinOnly,
    NoNearCapNotice,
    WatcherCountsWriter,
}

#[cfg(any(test, feature = "differential"))]
fn planted(p: Plant) -> bool {
    use crate::differential::{client_mutant, ClientMutant};
    let m = match p {
        Plant::ReusePersonHeld => ClientMutant::ReusePersonHeld,
        Plant::ReuseParked => ClientMutant::ReuseParked,
        Plant::HandoverWithoutClose => ClientMutant::HandoverWithoutClose,
        Plant::StaleReclaimTakesOwn => ClientMutant::StaleReclaimTakesOwn,
        Plant::ExpiryWithoutCap => ClientMutant::ExpiryWithoutCap,
        Plant::NoLoadEnd => ClientMutant::NoLoadEnd,
        Plant::ExpiryIgnored => ClientMutant::ExpiryIgnored,
        Plant::ReclaimNeverKeeps => ClientMutant::ReclaimNeverKeeps,
        Plant::CapUnchecked => ClientMutant::CapUnchecked,
        Plant::CapTakesNewestParked => ClientMutant::CapTakesNewestParked,
        Plant::CapOnExternal => ClientMutant::CapOnExternal,
        Plant::CapJoinOnly => ClientMutant::CapJoinOnly,
        Plant::NoNearCapNotice => ClientMutant::NoNearCapNotice,
        Plant::WatcherCountsWriter => ClientMutant::WatcherCountsWriter,
    };
    client_mutant() == Some(m)
}

#[cfg(not(any(test, feature = "differential")))]
fn planted(_: Plant) -> bool {
    false
}

fn any_planted() -> bool {
    planted(Plant::ReusePersonHeld)
        || planted(Plant::ReuseParked)
        || planted(Plant::HandoverWithoutClose)
}

/// The planted fault `ClientMutant::HandoverWithoutClose`: a hand-over keeps the holder.
pub(super) fn planted_handover_keeps_holder() -> bool {
    planted(Plant::HandoverWithoutClose)
}

/// The planted fault `ClientMutant::WatcherCountsWriter`: a connection watching past
/// `participants_max` with edit rights takes and counts a writer as one that could write.
pub(super) fn planted_watcher_counts_writer() -> bool {
    planted(Plant::WatcherCountsWriter)
}

/// The planted fault `ClientMutant::CapOnExternal`: the outside participant's first writer is
/// refused at the cap.
pub(super) fn planted_cap_on_external() -> bool {
    planted(Plant::CapOnExternal)
}
