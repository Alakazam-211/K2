//! `SyncState`, `SyncBoot` and the pure planner (S25 §13.1–§13.2).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sew_proto::gateway::server::Ready;
use sew_proto::id::Id;

/// The `format` of a stored [`SyncState`].
pub const SYNC_STATE_FORMAT: u32 = 1;

/// Where the client is: install, epoch, member log position and per-channel cursors.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SyncState {
    /// Always [`SYNC_STATE_FORMAT`].
    pub format: u32,
    /// The install the cursors belong to.
    pub install_id: Option<Id>,
    /// Opaque (an i64 as a decimal string, rev 8); any change means "drop everything".
    pub sync_epoch: Option<String>,
    /// The member the cursors belong to.
    pub member_id: Option<Id>,
    /// The member change-log position.
    pub member_seq: u64,
    /// Per-channel cursors.
    pub channels: BTreeMap<Id, ChannelCursor>,
}

impl Default for SyncState {
    fn default() -> Self {
        SyncState {
            format: SYNC_STATE_FORMAT,
            install_id: None,
            sync_epoch: None,
            member_id: None,
            member_seq: 0,
            channels: BTreeMap::new(),
        }
    }
}

/// One channel's cursor: the change-log seq the client has applied, and the newest message it
/// holds.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ChannelCursor {
    /// The channel change-log seq applied.
    pub seq: u64,
    /// The newest message held.
    pub last_message_id: Option<Id>,
}

/// What the server says now: from READY or `GET /api/v1/me/sync` (gap S25-G2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncBoot {
    /// The install.
    pub install_id: Id,
    /// The install's sync epoch (opaque).
    pub sync_epoch: String,
    /// The member.
    pub member_id: Id,
    /// The member change-log head.
    pub member_seq: u64,
    /// Every channel the member can see now, DMs included.
    pub channels: Vec<BootChannel>,
}

/// One channel's head at boot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootChannel {
    /// The channel.
    pub id: Id,
    /// Its change-log head.
    pub head_seq: u64,
    /// Its newest message.
    pub last_message_id: Option<Id>,
}

impl SyncBoot {
    /// The boot `GET /api/v1/me/sync` describes (S14, agents without a socket).
    pub fn from_me_sync(s: &sew_proto::api::read_state::MeSync) -> SyncBoot {
        SyncBoot {
            install_id: s.install_id,
            sync_epoch: s.sync_epoch.clone(),
            member_id: s.member_id,
            member_seq: s.member_seq,
            channels: s
                .channels
                .iter()
                .map(|c| BootChannel {
                    id: c.id,
                    head_seq: c.head_seq,
                    last_message_id: c.last_message_id,
                })
                .collect(),
        }
    }

    /// The boot a READY describes.
    pub fn from_ready(r: &Ready) -> SyncBoot {
        SyncBoot {
            install_id: r.install.id,
            sync_epoch: r.install.sync_epoch.clone(),
            member_id: r.member.id,
            member_seq: r.member_seq,
            channels: r
                .channels
                .iter()
                .map(|c| BootChannel {
                    id: c.id,
                    head_seq: c.head_seq,
                    last_message_id: c.last_message_id,
                })
                .collect(),
        }
    }
}

/// What catch-up must do, in order: drop, member log, fresh loads, pulls.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncPlan {
    /// Drop everything held (install, member or epoch changed, or the server went back).
    pub drop_all: bool,
    /// Drop these channels (left, removed, no longer visible; or head behind the cursor).
    pub drop_channels: Vec<Id>,
    /// Pull `/me/changes` from here, if set.
    pub member_since: Option<u64>,
    /// `(channel, since)`: pull `/channels/{id}/changes?since=`; busy channels first.
    pub pulls: Vec<(Id, u64)>,
    /// Load the latest page; the cursor becomes the boot head.
    pub fresh: Vec<Id>,
}

impl SyncState {
    /// Whether nothing was ever synced.
    pub fn is_empty(&self) -> bool {
        self.install_id.is_none()
            && self.member_id.is_none()
            && self.sync_epoch.is_none()
            && self.member_seq == 0
            && self.channels.is_empty()
    }

    /// Plans catch-up against `boot` (pure). Rules, in order (S25 §13.2):
    /// 1. a stored install, member or epoch that differs → `drop_all`, every channel fresh;
    /// 2. an empty state → every channel fresh (no drop);
    /// 3. a member seq that went backward → as rule 1;
    /// 4. a member seq that advanced → `member_since`;
    /// 5. stored channels missing from boot → dropped;
    /// 6. boot channels not stored → fresh;
    /// 7. head ahead → pull; equal → nothing; behind → drop and fresh;
    /// 8. pulls ordered by `last_message_id` descending.
    pub fn plan(&self, boot: &SyncBoot) -> SyncPlan {
        let all_fresh = |drop_all: bool| SyncPlan {
            drop_all,
            drop_channels: Vec::new(),
            member_since: None,
            pulls: Vec::new(),
            fresh: boot.channels.iter().map(|c| c.id).collect(),
        };
        let differs = self.install_id.is_some_and(|i| i != boot.install_id)
            || self.member_id.is_some_and(|m| m != boot.member_id)
            || self
                .sync_epoch
                .as_deref()
                .is_some_and(|e| e != boot.sync_epoch);
        if differs {
            return all_fresh(true);
        }
        if self.is_empty() {
            return all_fresh(false);
        }
        if boot.member_seq < self.member_seq {
            tracing::warn!(
                stored = self.member_seq,
                boot = boot.member_seq,
                "sew-client: the member log went backward without an epoch change; dropping all"
            );
            return all_fresh(true);
        }
        let mut plan = SyncPlan {
            member_since: (boot.member_seq > self.member_seq).then_some(self.member_seq),
            ..SyncPlan::default()
        };
        let in_boot: BTreeSet<Id> = boot.channels.iter().map(|c| c.id).collect();
        plan.drop_channels = self
            .channels
            .keys()
            .filter(|id| !in_boot.contains(id))
            .copied()
            .collect();
        let mut pulls: Vec<(Id, u64, Option<Id>)> = Vec::new();
        for c in &boot.channels {
            match self.channels.get(&c.id) {
                None => plan.fresh.push(c.id),
                Some(cur) if c.head_seq > cur.seq => pulls.push((c.id, cur.seq, c.last_message_id)),
                Some(cur) if c.head_seq < cur.seq => {
                    plan.drop_channels.push(c.id);
                    plan.fresh.push(c.id);
                }
                Some(_) => {}
            }
        }
        // Busy channels first: newest last message first; channels without one last.
        pulls.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
        plan.pulls = pulls
            .into_iter()
            .map(|(id, since, _)| (id, since))
            .collect();
        plan
    }

    /// Adopts `boot`'s install, member and epoch (after `drop_all`, or on first sync).
    pub fn adopt(&mut self, boot: &SyncBoot) {
        self.format = SYNC_STATE_FORMAT;
        self.install_id = Some(boot.install_id);
        self.member_id = Some(boot.member_id);
        self.sync_epoch = Some(boot.sync_epoch.clone());
    }
}
