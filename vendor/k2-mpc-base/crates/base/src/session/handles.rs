//! Opaque handles for App viewers (K5): a participant whose kind is
//! [`ParticipantKind::App`](super::ParticipantKind::App) never sees another participant's pid,
//! nor its own. Every pid in what the kit hands such a viewer (the roster and its push,
//! change events, blame, the log, claims, the activity ring, heads-up notes, and the
//! refusals that name someone) is replaced by a handle, `p1`, `p2`...
//!
//! Handles are per doc and per viewer: numbered in the order that viewer first sees each pid,
//! the same for the same pid across the viewer's reconnects while the doc is loaded, and
//! independent between viewers (one viewer's `p2` says nothing about another's). The kit's
//! own markers (`kit:import`, `kit:external`, `kit:unknown`, an erased participant's
//! pseudonym) are not participants and pass unchanged.

use std::collections::HashMap;

use super::awareness::{ActivityEntry, ClaimInfo, Note, ReservationInfo};
use super::error::SessionError;
use super::live::{Blame, ChangeEvent, LogItem};
use super::participant::{RosterEntry, RESERVED_PID_PREFIX};
use super::roster::{RosterEvent, RosterMember, Who};

/// A handle's prefix: `p1`, `p2`...
const PREFIX: char = 'p';
/// What an App's filter by an unknown name matches: no participant (a reserved pid).
const NOBODY: &str = "kit:nobody";

/// Every App viewer's handles in one doc. Session state.
#[derive(Default)]
pub(crate) struct Handles {
    /// viewer pid -> the pids it has seen, in order (handle `p{k+1}` is `[k]`).
    by_viewer: HashMap<String, Vec<String>>,
}

impl Handles {
    /// The lens for `viewer` (an App participant's pid).
    pub fn lens(&mut self, viewer: &str) -> Lens<'_> {
        Lens {
            seen: self.by_viewer.entry(viewer.to_owned()).or_default(),
        }
    }

    /// Participant erasure: forget the pid everywhere (as a viewer and as seen).
    pub fn forget(&mut self, pid: &str) {
        self.by_viewer.remove(pid);
        // keep positions (handles stay stable for the others); blank the pid
        for seen in self.by_viewer.values_mut() {
            for p in seen.iter_mut().filter(|p| *p == pid) {
                p.clear();
            }
        }
    }
}

/// What one App viewer may see: pids become its handles.
pub(crate) struct Lens<'a> {
    seen: &'a mut Vec<String>,
}

impl Lens<'_> {
    /// The handle for `pid` (minted on first sight). Kit markers pass unchanged.
    pub fn pid(&mut self, pid: &str) -> String {
        if pid.is_empty() || pid.starts_with(RESERVED_PID_PREFIX) {
            return pid.to_owned();
        }
        let k = match self.seen.iter().position(|p| p == pid) {
            Some(k) => k,
            None => {
                self.seen.push(pid.to_owned());
                self.seen.len() - 1
            }
        };
        format!("{PREFIX}{}", k + 1)
    }

    fn opt(&mut self, pid: Option<String>) -> Option<String> {
        pid.map(|p| self.pid(&p))
    }

    fn all(&mut self, pids: Vec<String>) -> Vec<String> {
        pids.into_iter().map(|p| self.pid(&p)).collect()
    }

    /// The pid behind one of this viewer's handles (an App's `log --by p2`). Anything else
    /// (a raw pid, a handle it never saw) names nobody: an App can't probe for pids.
    pub fn unpid(&self, s: &str) -> String {
        s.strip_prefix(PREFIX)
            .and_then(|n| n.parse::<usize>().ok())
            .and_then(|n| n.checked_sub(1))
            .and_then(|k| self.seen.get(k))
            .filter(|p| !p.is_empty())
            .cloned()
            .unwrap_or_else(|| NOBODY.to_owned())
    }

    pub fn who(&mut self, w: Who) -> Who {
        match w {
            Who::Pid(p) => Who::Handle(self.pid(&p)),
            other => other,
        }
    }

    pub fn entry(&mut self, mut e: RosterEntry) -> RosterEntry {
        e.participant.pid = self.pid(&e.participant.pid);
        e.participant.on_behalf_of = self.opt(e.participant.on_behalf_of.take());
        e.claims = e.claims.into_iter().map(|c| self.claim(c)).collect();
        e
    }

    pub fn claim(&mut self, mut c: ClaimInfo) -> ClaimInfo {
        c.pid = self.pid(&c.pid);
        c.waiters = self.all(std::mem::take(&mut c.waiters));
        c
    }

    pub fn reservation(&mut self, mut r: ReservationInfo) -> ReservationInfo {
        r.pid = self.pid(&r.pid);
        r
    }

    pub fn note(&mut self, mut n: Note) -> Note {
        n.pid = self.pid(&n.pid);
        n
    }

    pub fn activity(&mut self, mut a: ActivityEntry) -> ActivityEntry {
        a.pid = self.pid(&a.pid);
        a
    }

    pub fn change(&mut self, mut c: ChangeEvent) -> ChangeEvent {
        c.pid = self.pid(&c.pid);
        c.on_behalf_of = self.opt(c.on_behalf_of.take());
        c
    }

    pub fn blame(&mut self, mut b: Blame) -> Blame {
        for l in &mut b.lines {
            l.author = self.pid(&l.author);
            l.on_behalf_of = self.opt(l.on_behalf_of.take());
        }
        b
    }

    pub fn log_item(&mut self, item: LogItem) -> LogItem {
        match item {
            LogItem::Change { mut info, approval } => {
                info.author = self.pid(&info.author);
                info.on_behalf_of = self.opt(info.on_behalf_of.take());
                LogItem::Change { info, approval }
            }
            LogItem::Version(mut v) => {
                v.authors = self.all(std::mem::take(&mut v.authors));
                LogItem::Version(v)
            }
        }
    }

    pub fn approval(&mut self, mut a: crate::store::Approval) -> crate::store::Approval {
        a.by = self.opt(a.by.take());
        a.author = self.pid(&a.author);
        a.on_behalf_of = self.pid(&a.on_behalf_of);
        a
    }

    // ----- tier 0 (blob docs)

    pub fn lease(&mut self, mut l: super::tier0::LeaseInfo) -> super::tier0::LeaseInfo {
        l.pid = self.pid(&l.pid);
        l
    }

    pub fn blob_version(&mut self, mut v: crate::store::BlobVersion) -> crate::store::BlobVersion {
        v.author = self.pid(&v.author);
        v.on_behalf_of = self.opt(v.on_behalf_of.take());
        v
    }

    pub fn blob_log_entry(
        &mut self,
        mut e: super::tier0::BlobLogEntry,
    ) -> super::tier0::BlobLogEntry {
        e.author = self.pid(&e.author);
        e.on_behalf_of = self.opt(e.on_behalf_of.take());
        e
    }

    pub fn blob_blame(&mut self, mut b: super::tier0::BlobBlame) -> super::tier0::BlobBlame {
        b.current = self.blob_version(b.current);
        for a in &mut b.authors {
            a.pid = self.pid(&a.pid);
        }
        b
    }

    pub fn blob_approval(
        &mut self,
        mut a: super::tier0::BlobApproval,
    ) -> super::tier0::BlobApproval {
        a.approval = self.approval(a.approval);
        a
    }

    /// A refusal that names another participant (a claim's holder).
    pub fn error(&mut self, e: SessionError) -> SessionError {
        match e {
            SessionError::Claimed {
                id,
                by,
                name,
                lines,
                remaining_ms,
                note,
            } => SessionError::Claimed {
                id,
                by: self.pid(&by),
                name,
                lines,
                remaining_ms,
                note,
            },
            SessionError::Leased {
                holder,
                name,
                expires_ms,
            } => SessionError::Leased {
                holder: self.pid(&holder),
                name,
                expires_ms,
            },
            other => other,
        }
    }

    pub fn roster_event(&mut self, ev: RosterEvent) -> RosterEvent {
        match ev {
            RosterEvent::Snapshot {
                version,
                members,
                watching,
            } => RosterEvent::Snapshot {
                version,
                members: members
                    .into_iter()
                    .map(|m| RosterMember {
                        who: self.who(m.who),
                        entry: self.entry(m.entry),
                        rights: m.rights,
                    })
                    .collect(),
                watching,
            },
            RosterEvent::Joined {
                who,
                mut participant,
                rights,
            } => {
                participant.pid = self.pid(&participant.pid);
                participant.on_behalf_of = self.opt(participant.on_behalf_of.take());
                RosterEvent::Joined {
                    who: self.who(who),
                    participant,
                    rights,
                }
            }
            RosterEvent::Left { who, reason } => RosterEvent::Left {
                who: self.who(who),
                reason,
            },
            RosterEvent::Rights { who, rights } => RosterEvent::Rights {
                who: self.who(who),
                rights,
            },
            RosterEvent::Role { who, role } => RosterEvent::Role {
                who: self.who(who),
                role,
            },
            RosterEvent::Away { who, away } => RosterEvent::Away {
                who: self.who(who),
                away,
            },
            RosterEvent::Cursor {
                who,
                cursor,
                version,
            } => RosterEvent::Cursor {
                who: self.who(who),
                cursor,
                version,
            },
            RosterEvent::ClaimStarted {
                who,
                claim,
                lines,
                note,
                hold,
                for_edit,
                expires_ms,
                version,
            } => RosterEvent::ClaimStarted {
                who: self.who(who),
                claim,
                lines,
                note,
                hold,
                for_edit,
                expires_ms,
                version,
            },
            RosterEvent::ClaimEnded {
                who,
                claim,
                how,
                lines,
                version,
            } => RosterEvent::ClaimEnded {
                who: self.who(who),
                claim,
                how,
                lines,
                version,
            },
            RosterEvent::Note {
                to,
                from,
                kind,
                lines,
                claim,
            } => RosterEvent::Note {
                to: self.who(to),
                from: self.who(from),
                kind,
                lines,
                claim,
            },
            w @ RosterEvent::Watching { .. } => w,
        }
    }
}
