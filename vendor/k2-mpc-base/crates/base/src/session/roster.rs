//! The roster pushed to subscribers ([`super::Session::subscribe_roster`]): a full
//! [`RosterEvent::Snapshot`] first, then deltas, coalesced at
//! [`super::Config::roster_interval_ms`].
//!
//! Nothing here is a second copy of presence. Joins, leaves, claim starts and ends and
//! heads-up notes are recorded as they happen (they are never coalesced away); role, rights,
//! away and the caret are read from the session's own presence at each flush and compared
//! with what subscribers were last told (so a caret moved ten times in an interval is one
//! event, its last place).
//!
//! A subscriber's queue holds [`super::Config::roster_queue`] events. One that falls that
//! far behind loses the deltas it can't take and gets a fresh `Snapshot` as soon as it has
//! room again: its memory is bounded, and it is never left with a stale roster.

use std::collections::HashMap;
use std::fmt;
use std::sync::mpsc::{SyncSender, TrySendError};

use super::awareness::{ClaimEnd, ClaimId, NoteKind, Overwrite};
use super::handles::Handles;
use super::host::{LeaveReason, Rights};
use super::participant::{ConnId, CursorAt, Participant, Role, RosterEntry};
use super::position::LineRange;
use crate::store::Seq;

/// Who an event is about: the host's participant id, or, for a viewer whose participant
/// kind is [`super::ParticipantKind::App`], an opaque per-doc handle (`p1`, `p2`...) that
/// never reveals the pid. The enum is `#[non_exhaustive]`: match with a wildcard arm, and
/// use [`fmt::Display`] to show it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Who {
    /// The host-stamped participant id ([`Participant::pid`]).
    Pid(String),
    /// An App viewer's handle for a participant (`p1`, `p2`...): per doc and per viewer,
    /// stable across the viewer's reconnects while the doc is loaded.
    Handle(String),
}

impl Who {
    /// The participant id, when this `Who` carries one.
    pub fn pid(&self) -> Option<&str> {
        match self {
            Who::Pid(p) => Some(p),
            Who::Handle(_) => None,
        }
    }

    /// The handle, when this `Who` is one (an App viewer's).
    pub fn handle(&self) -> Option<&str> {
        match self {
            Who::Handle(h) => Some(h),
            Who::Pid(_) => None,
        }
    }

    pub(crate) fn of(pid: &str) -> Self {
        Who::Pid(pid.to_owned())
    }
}

impl fmt::Display for Who {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Who::Pid(p) | Who::Handle(p) => f.write_str(p),
        }
    }
}

/// One participant in a [`RosterEvent::Snapshot`]: the [`super::Session::roster`] row, with
/// its rights.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RosterMember {
    pub who: Who,
    pub entry: RosterEntry,
    pub rights: Rights,
}

/// One change to a doc's roster, as [`super::Session::subscribe_roster`] delivers it.
///
/// The first event on every subscription is a [`RosterEvent::Snapshot`]; after it, deltas
/// in the order they happened (within a flush: the recorded events first, then the role,
/// rights, away and caret changes in roster order). A later `Snapshot` replaces everything
/// before it: it comes when the receiver fell [`super::Config::roster_queue`] events
/// behind. Every subscriber of a doc sees the same events in the same order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RosterEvent {
    /// The whole roster now, as [`super::Session::roster`] orders it, at doc `version`, and
    /// how many participants are watching past [`super::Config::participants_max`] (read-only,
    /// shown only as this count). Every member's caret ([`RosterEntry::cursor`]) is in bytes
    /// of the root's text at `version`: the rows and the version are read together.
    Snapshot {
        version: Seq,
        members: Vec<RosterMember>,
        watching: usize,
        /// This subscriber's own waiting [`Overwrite`] notes (a push doesn't consume one).
        overwritten: Vec<Overwrite>,
    },
    /// A participant's first connection joined. They start as a viewer, not away, with no
    /// caret; any of those that differ follow as their own events in the same flush.
    Joined {
        who: Who,
        participant: Participant,
        rights: Rights,
    },
    /// A participant's last connection left.
    Left { who: Who, reason: LeaveReason },
    /// Their rights changed (the host's re-check, or any call that re-asked).
    Rights { who: Who, rights: Rights },
    /// Editor or viewer ([`super::Config::editor_window_ms`]).
    Role { who: Who, role: Role },
    /// Away (every connection hidden for [`super::Config::away_after_ms`]) or back.
    Away { who: Who, away: bool },
    /// Their caret: anchor and head, in bytes and `line:col`, or gone. Coalesced: one per
    /// participant per flush, the last place (carried through other people's edits too).
    Cursor {
        who: Who,
        cursor: Option<CursorAt>,
        /// The doc's version the caret is in: its byte offsets (and `line:col`) are positions
        /// in the root's text at this version, read together with the caret at delivery.
        /// A client whose text is at another version maps them through the changes between
        /// (e.g. [`super::Session::events_since`]).
        version: Seq,
    },
    /// A claim was taken (explicitly or by a read's `--for-edit`). Never coalesced.
    ClaimStarted {
        who: Who,
        claim: ClaimId,
        lines: LineRange,
        note: Option<String>,
        hold: bool,
        for_edit: bool,
        expires_ms: u64,
        /// The doc's version when it was taken.
        version: Seq,
    },
    /// A claim ended, and how. Never coalesced.
    ClaimEnded {
        who: Who,
        claim: ClaimId,
        how: ClaimEnd,
        /// Its lines when it ended.
        lines: LineRange,
        /// The doc's version when it ended.
        version: Seq,
    },
    /// A heads-up note was delivered to `to` (on a read or an edit): `from` is viewing,
    /// edited, claimed, or has a caret or selection on lines `to` touched.
    Note {
        to: Who,
        from: Who,
        kind: NoteKind,
        lines: LineRange,
        claim: Option<ClaimId>,
    },
    /// An outside save may have removed `to`'s text ([`Overwrite`]). Sent only to `to`'s own
    /// subscriptions, never coalesced. The note waits too, until `take_notes` or a verb delivers
    /// it, so a slow receiver that gets a fresh `Snapshot` still finds it there.
    Overwritten { to: Who, overwrite: Overwrite },
    /// The host closed this doc's live session (`reason`, its code): the last event of every
    /// subscription, which then ends. Later calls fail with
    /// [`super::SessionError::Closed`].
    Closed { reason: &'static str },
    /// The number of participants watching past [`super::Config::participants_max`] changed
    /// (they carry no presence detail; this count is all the roster shows of them).
    Watching { count: usize },
}

/// What subscribers were last told about one participant.
#[derive(Clone)]
struct Told {
    who: Who,
    rights: Rights,
    role: Role,
    away: bool,
    cursor: Option<CursorAt>,
}

struct Sub {
    conn: ConnId,
    /// Whose it is: an `Overwritten` event goes only to its author's subscriptions.
    pid: String,
    /// An App viewer's pid: its events go through its handle lens.
    app: Option<String>,
    tx: SyncSender<RosterEvent>,
    /// Owes a fresh snapshot (it fell behind).
    resync: bool,
}

/// A session's roster subscribers and what they were last told. Session state.
#[derive(Default)]
pub(crate) struct RosterHub {
    subs: Vec<Sub>,
    /// Recorded events since the last flush (kept only while someone subscribes).
    pending: Vec<RosterEvent>,
    /// More than a queue's worth was recorded before a flush: every subscriber resyncs.
    overflow: bool,
    told: Vec<Told>,
    told_watching: usize,
    last_flush_ms: u64,
}

/// One participant as the flush reads it from presence.
pub(crate) struct Row {
    pub entry: RosterEntry,
    pub rights: Rights,
}

impl RosterHub {
    pub fn is_empty(&self) -> bool {
        self.subs.is_empty()
    }

    /// Record an event that must not be coalesced away (bounded by `cap`).
    pub fn record(&mut self, ev: RosterEvent, cap: usize) {
        if self.subs.is_empty() || self.overflow {
            return;
        }
        if self.pending.len() >= cap.max(1) {
            self.pending.clear();
            self.overflow = true;
            return;
        }
        self.pending.push(ev);
    }

    /// A new subscriber: its snapshot first. The caller has just flushed (forced), so the
    /// subscriber starts exactly where the others are.
    #[allow(clippy::too_many_arguments)]
    pub fn subscribe(
        &mut self,
        conn: ConnId,
        pid: String,
        overwritten: Vec<Overwrite>,
        app: Option<String>,
        tx: SyncSender<RosterEvent>,
        now: u64,
        version: Seq,
        rows: Vec<Row>,
        watching: usize,
        handles: &mut Handles,
    ) {
        let mut snap = snapshot(version, &rows, watching);
        if let RosterEvent::Snapshot { overwritten: o, .. } = &mut snap {
            *o = overwritten;
        }
        if let Some(v) = &app {
            snap = handles.lens(v).roster_event(snap);
        }
        if self.subs.is_empty() {
            self.told = rows.iter().map(told).collect();
            self.told_watching = watching;
            self.pending.clear();
            self.overflow = false;
        }
        let resync = tx.try_send(snap).is_err();
        self.subs.push(Sub {
            conn,
            pid,
            app,
            tx,
            resync,
        });
        // the snapshot is a delivery: the next is due an interval from now
        self.last_flush_ms = now;
    }

    /// Close the subscriptions a connection opened (it left).
    pub fn drop_conn(&mut self, conn: ConnId) {
        self.subs.retain(|s| s.conn != conn);
        if self.subs.is_empty() {
            self.reset();
        }
    }

    /// Close every subscription (the session closed).
    pub fn close(&mut self) {
        self.subs.clear();
        self.reset();
    }

    /// Send `ev` to every subscription, as its last event, then end them all. A subscriber whose
    /// queue is full doesn't get it; its channel still ends.
    pub fn end_with(&mut self, ev: RosterEvent) {
        for s in &self.subs {
            let _ = s.tx.try_send(ev.clone());
        }
        self.close();
    }

    /// The events recorded since the last flush.
    pub fn pending_mut(&mut self) -> &mut Vec<RosterEvent> {
        &mut self.pending
    }

    /// Whether the next flush sends anyone a snapshot.
    pub fn owes_snapshot(&self) -> bool {
        self.overflow || self.subs.iter().any(|s| s.resync)
    }

    fn reset(&mut self) {
        self.pending.clear();
        self.told.clear();
        self.overflow = false;
    }

    /// Whether a flush is due at `now` (interval passed, and something to do).
    pub fn due(&self, now: u64, interval: u64) -> bool {
        !self.subs.is_empty() && now.saturating_sub(self.last_flush_ms) >= interval
    }

    /// Deliver: the recorded events, then the changes `rows` (presence now, in roster
    /// order) show against what was last told; fresh snapshots to subscribers that owe one.
    /// `version` is the doc's version `rows` were read at (under the same lock): the carets
    /// in `rows` are in its text, so it is the version on every `Cursor` and `Snapshot`.
    pub fn flush(
        &mut self,
        now: u64,
        version: Seq,
        rows: Vec<Row>,
        watching: usize,
        handles: &mut Handles,
        waiting: &HashMap<String, Vec<Overwrite>>,
    ) {
        self.last_flush_ms = now;
        if self.subs.is_empty() {
            return;
        }
        let mut batch = Vec::new();
        if self.overflow {
            self.overflow = false;
            for s in &mut self.subs {
                s.resync = true;
            }
            self.told = rows.iter().map(told).collect();
            self.told_watching = watching;
        } else {
            for ev in self.pending.drain(..) {
                match &ev {
                    RosterEvent::Joined { who, rights, .. } => {
                        self.told.retain(|t| &t.who != who);
                        self.told.push(Told {
                            who: who.clone(),
                            rights: *rights,
                            role: Role::Viewer,
                            away: false,
                            cursor: None,
                        });
                    }
                    RosterEvent::Left { who, .. } => self.told.retain(|t| &t.who != who),
                    _ => {}
                }
                batch.push(ev);
            }
            for r in &rows {
                let who = Who::of(&r.entry.participant.pid);
                let k = match self.told.iter().position(|t| t.who == who) {
                    Some(k) => k,
                    None => {
                        // present but never announced (cannot happen: joins are recorded);
                        // announce it rather than stay silent
                        batch.push(RosterEvent::Joined {
                            who: who.clone(),
                            participant: r.entry.participant.clone(),
                            rights: r.rights,
                        });
                        self.told.push(Told {
                            who: who.clone(),
                            rights: r.rights,
                            role: Role::Viewer,
                            away: false,
                            cursor: None,
                        });
                        self.told.len() - 1
                    }
                };
                let t = &mut self.told[k];
                if t.rights != r.rights {
                    t.rights = r.rights;
                    batch.push(RosterEvent::Rights {
                        who: who.clone(),
                        rights: r.rights,
                    });
                }
                if t.role != r.entry.role {
                    t.role = r.entry.role;
                    batch.push(RosterEvent::Role {
                        who: who.clone(),
                        role: r.entry.role,
                    });
                }
                if t.away != r.entry.away {
                    t.away = r.entry.away;
                    batch.push(RosterEvent::Away {
                        who: who.clone(),
                        away: r.entry.away,
                    });
                }
                if t.cursor != r.entry.cursor {
                    t.cursor = r.entry.cursor.clone();
                    batch.push(RosterEvent::Cursor {
                        who,
                        cursor: r.entry.cursor.clone(),
                        version,
                    });
                }
            }
        }
        if !self.overflow && watching != self.told_watching {
            self.told_watching = watching;
            batch.push(RosterEvent::Watching { count: watching });
        }
        let mut snap: Option<RosterEvent> = None;
        self.subs.retain_mut(|s| {
            let mut lens = s.app.as_deref().map(|v| handles.lens(v));
            if s.resync {
                let mut ev = snap
                    .get_or_insert_with(|| snapshot(version, &rows, watching))
                    .clone();
                if let RosterEvent::Snapshot { overwritten, .. } = &mut ev {
                    *overwritten = waiting.get(&s.pid).cloned().unwrap_or_default();
                }
                if let Some(l) = lens.as_mut() {
                    ev = l.roster_event(ev);
                }
                return match s.tx.try_send(ev) {
                    Ok(()) => {
                        s.resync = false;
                        true
                    }
                    Err(TrySendError::Full(_)) => true,
                    Err(TrySendError::Disconnected(_)) => false,
                };
            }
            for ev in &batch {
                if matches!(ev, RosterEvent::Overwritten { to, .. } if *to != Who::of(&s.pid)) {
                    continue;
                }
                let ev = match lens.as_mut() {
                    Some(l) => l.roster_event(ev.clone()),
                    None => ev.clone(),
                };
                match s.tx.try_send(ev) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        s.resync = true;
                        return true;
                    }
                    Err(TrySendError::Disconnected(_)) => return false,
                }
            }
            true
        });
        if self.subs.is_empty() {
            self.reset();
        }
    }
}

fn told(r: &Row) -> Told {
    Told {
        who: Who::of(&r.entry.participant.pid),
        rights: r.rights,
        role: r.entry.role,
        away: r.entry.away,
        cursor: r.entry.cursor.clone(),
    }
}

fn snapshot(version: Seq, rows: &[Row], watching: usize) -> RosterEvent {
    RosterEvent::Snapshot {
        watching,
        version,
        overwritten: Vec::new(),
        members: rows
            .iter()
            .map(|r| RosterMember {
                who: Who::of(&r.entry.participant.pid),
                entry: r.entry.clone(),
                rights: r.rights,
            })
            .collect(),
    }
}
