//! Grouped commits ([`super::CommitDurability::Grouped`]): which commits wait for the disk.
//!
//! At [`super::CommitDurability::Immediate`] (the default) every commit waits for the disk, as
//! it always has: nothing here changes what such a store does or writes.
//!
//! Grouped, a commit is made without waiting for the disk (applied, visible to every reader
//! and placed in the store's one order of commits), and the group of such commits is made
//! durable by one commit that waits: the group's last, whichever comes first of
//! - the commit that would put the group past `max_changes` commits;
//! - the first commit, or the first [`super::DocStore::tick_group`], `max_ms` or more after the
//!   group's first commit, on the host's clock ([`super::DocStore::set_clock`]; without one,
//!   only the count and the barriers bound a group);
//! - a barrier: a commit something outside the store depends on (the write-back's record
//!   before the file is written, a cut, a delete, a purge, a demotion or promotion), made by
//!   [`super::DocStore::begin_write_durable`], and [`super::DocStore::flush`] (an unload, a
//!   close, a release, the store's own close).
//!
//! The store library keeps a commit that didn't wait only until a later commit that waits:
//! an abrupt stop or a power loss reopens the store at its last durable commit, a prefix of
//! the commits, never a torn one.
//!
//! **A failed commit, grouped.** When a commit fails at the disk (or the store library refuses
//! every call after an earlier failure), the commits not yet durable are gone from the disk
//! while live docs still hold them: as after an abrupt stop, inside the process. The store
//! refuses every transaction from then on ([`super::DocStore::rollback_pending`]) until the
//! session manager reopens it at its durable prefix and reloads every live doc from it
//! ([`crate::session::SessionManager::tick`], [`crate::session::SessionManager::flush`]). At
//! `Immediate` a failed commit lost nothing acknowledged, and the store behaves as it always
//! has.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, Weak};

use super::CommitDurability;

/// The host's clock, ms ([`super::DocStore::set_clock`]).
pub(crate) type Clock = dyn Fn() -> u64 + Send + Sync;

/// The commits not yet durable.
#[derive(Debug, Default)]
struct Group {
    /// How many.
    pending: u32,
    /// When the first was made (the host's clock), if a clock was there.
    first_ms: Option<u64>,
}

/// The store's commits in their one order ([`super::DocStore::commits`]), how many of them are
/// on disk ([`super::DocStore::durable_through`]), and the group not yet durable.
#[derive(Default)]
pub(crate) struct Commits {
    made: AtomicU64,
    durable: AtomicU64,
    group: Mutex<Group>,
    clock: Mutex<Option<Weak<Clock>>>,
    /// A grouped commit failed at the disk: the store waits to be reopened (module docs).
    failed: AtomicBool,
    /// The commit being made raised a doc's seq floor (`seq_floor.rs`): it waits for the disk.
    floor_raised: AtomicBool,
    /// The store holds lost versions (`seq_floor.rs`): reads of a version check them.
    lost_known: AtomicBool,
}

impl std::fmt::Debug for Commits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Commits")
            .field("made", &self.made())
            .field("durable", &self.durable())
            .field("pending", &self.pending())
            .finish()
    }
}

impl Commits {
    pub(super) fn made(&self) -> u64 {
        self.made.load(Ordering::SeqCst)
    }

    pub(super) fn durable(&self) -> u64 {
        self.durable.load(Ordering::SeqCst)
    }

    /// The commits made since the last durable one.
    pub(super) fn pending(&self) -> u32 {
        self.group_lock().pending
    }

    fn group_lock(&self) -> std::sync::MutexGuard<'_, Group> {
        self.group.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(super) fn set_clock(&self, clock: Weak<Clock>) {
        *self.clock.lock().unwrap_or_else(|p| p.into_inner()) = Some(clock);
    }

    fn now(&self) -> Option<u64> {
        let c = self
            .clock
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .and_then(Weak::upgrade);
        c.map(|c| c())
    }

    /// Whether the group made at `now` is past `max_ms`.
    fn late(g: &Group, now: Option<u64>, max_ms: u64) -> bool {
        // the plant: the time bound never checked
        if crate::beta_plant::planted("group-ms-ignored") {
            return false;
        }
        matches!((g.first_ms, now), (Some(f), Some(n)) if n.saturating_sub(f) >= max_ms)
    }

    /// Whether the commit about to be made waits for the disk (`commit` the store's setting,
    /// `barrier` a commit something outside the store depends on), and the clock's time.
    pub(super) fn next(&self, commit: CommitDurability, barrier: bool) -> (bool, Option<u64>) {
        let CommitDurability::Grouped {
            max_changes,
            max_ms,
        } = commit
        else {
            return (true, None);
        };
        let now = self.now();
        if barrier {
            return (true, now);
        }
        let g = self.group_lock();
        // the plant: the count never checked
        let full = g.pending.saturating_add(1) >= max_changes
            && !crate::beta_plant::planted("group-changes-ignored");
        (full || Self::late(&g, now, max_ms), now)
    }

    /// A commit made (`durable`: it waited for the disk), at `now`.
    pub(super) fn committed(&self, durable: bool, now: Option<u64>) {
        let made = self.made.fetch_add(1, Ordering::SeqCst) + 1;
        let mut g = self.group_lock();
        if durable {
            self.durable.fetch_max(made, Ordering::SeqCst);
            *g = Group::default();
        } else {
            g.pending += 1;
            if g.first_ms.is_none() {
                g.first_ms = now;
            }
        }
    }

    /// The commit being made raised a seq floor: it waits for the disk (`seq_floor.rs`).
    pub(super) fn raise_floor(&self) {
        self.floor_raised.store(true, Ordering::SeqCst);
    }

    /// Whether the commit being made raised a seq floor (and the mark cleared).
    pub(super) fn take_floor_raised(&self) -> bool {
        self.floor_raised.swap(false, Ordering::SeqCst)
    }

    pub(super) fn set_lost_known(&self) {
        self.lost_known.store(true, Ordering::SeqCst);
    }

    pub(super) fn lost_known(&self) -> bool {
        self.lost_known.load(Ordering::SeqCst)
    }

    /// A grouped commit failed at the disk (module docs).
    pub(super) fn fail(&self) {
        self.failed.store(true, Ordering::SeqCst);
    }

    pub(super) fn failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }

    /// The store reopened at its durable prefix: everything it holds is durable, no group
    /// waits, and it takes transactions again.
    pub(super) fn reopened(&self) {
        self.durable.fetch_max(self.made(), Ordering::SeqCst);
        *self.group_lock() = Group::default();
        self.failed.store(false, Ordering::SeqCst);
    }

    /// Whether the group is due at the tick (grouped, not empty, past `max_ms`).
    pub(super) fn due_at_tick(&self, commit: CommitDurability) -> bool {
        let CommitDurability::Grouped { max_ms, .. } = commit else {
            return false;
        };
        let now = self.now();
        let g = self.group_lock();
        g.pending > 0 && Self::late(&g, now, max_ms)
    }
}
