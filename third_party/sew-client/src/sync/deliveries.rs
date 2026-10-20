//! `DeliveryCursor`: the agent delivery feed cursor for K2 (S25 §13.5, contracts rev 36).

use serde::{Deserialize, Serialize};
use sew_proto::id::Id;
use sew_proto::mgmt::{DeliveriesQuery, Delivery};

use crate::error::ClientError;
use crate::mgmt::MgmtClient;

/// An agent's position in its delivery feed. The caller persists `(after, sync_epoch)` after it
/// has handled each step (rev 36: K2 saves before injecting).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct DeliveryCursor {
    /// The agent's member id.
    pub agent: Id,
    /// Deliveries with `seq > after` come next.
    pub after: u64,
    /// The install's sync epoch the cursor belongs to (opaque).
    pub sync_epoch: Option<String>,
}

/// One poll's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedStep {
    /// New deliveries (possibly none after the long-poll).
    Deliveries(Vec<Delivery>),
    /// The cursor jumped to the feed's `head` without delivering: old messages are never
    /// re-injected into agents (rev 36). K2 may show one "may have missed" notice.
    Reset {
        /// Why.
        reason: ResetReason,
        /// The new `after` (the feed head).
        head: u64,
    },
}

/// Why a cursor was reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResetReason {
    /// The install's `sync_epoch` changed (restore).
    EpochChanged,
    /// `410` with `reason = behind_floor`: entries were trimmed.
    BehindFloor,
    /// `410` with `reason = ahead_of_head`: the cursor is past the head (restore).
    AheadOfHead,
}

impl ResetReason {
    /// `epoch_changed`, `behind_floor`, `ahead_of_head`.
    pub fn as_str(self) -> &'static str {
        match self {
            ResetReason::EpochChanged => "epoch_changed",
            ResetReason::BehindFloor => "behind_floor",
            ResetReason::AheadOfHead => "ahead_of_head",
        }
    }
}

impl DeliveryCursor {
    /// A cursor at `after`.
    pub fn new(agent: Id, after: u64) -> DeliveryCursor {
        DeliveryCursor {
            agent,
            after,
            sync_epoch: None,
        }
    }

    /// One long-poll of `GET /mgmt/v1/agents/{agent}/deliveries?after=&timeout_ms=`.
    ///
    /// - A page whose `sync_epoch` differs from the stored one: its entries are discarded,
    ///   `after = page.head`, `Reset(EpochChanged)`.
    /// - `410` (`behind_floor` or `ahead_of_head`): `after = details.head` (rev 36; `floor` when
    ///   the server sent no head), `Reset`.
    /// - Otherwise `after` = the page's `next_after` (S9 §6.11: past skipped entries too; the
    ///   last entry's seq from a server that sends no `next_after`), and the entries.
    ///
    /// Cancel-safe: the cursor moves only when the poll returns.
    pub async fn poll(
        &mut self,
        mgmt: &MgmtClient,
        timeout_ms: u64,
    ) -> Result<FeedStep, ClientError> {
        let q = DeliveriesQuery {
            after: self.after,
            timeout_ms: Some(timeout_ms),
            limit: None,
        };
        match mgmt.agent_deliveries(self.agent, &q).await {
            Ok(page) => {
                let epoch = page.sync_epoch.clone();
                if let (Some(stored), Some(now)) = (&self.sync_epoch, &epoch)
                    && stored != now
                {
                    self.after = page.head;
                    self.sync_epoch = epoch;
                    return Ok(FeedStep::Reset {
                        reason: ResetReason::EpochChanged,
                        head: page.head,
                    });
                }
                if epoch.is_some() {
                    self.sync_epoch = epoch;
                }
                let next = page
                    .next_after
                    .max(page.entries.last().map_or(0, |d| d.seq));
                if next > self.after {
                    self.after = next;
                }
                Ok(FeedStep::Deliveries(page.entries))
            }
            Err(e) => {
                let Some(info) = e.api().and_then(|a| a.resync()) else {
                    return Err(e);
                };
                let reason = match info.reason.as_deref() {
                    Some("ahead_of_head") => ResetReason::AheadOfHead,
                    _ => ResetReason::BehindFloor,
                };
                let head = info.head.unwrap_or(info.floor);
                self.after = head;
                if info.sync_epoch.is_some() {
                    self.sync_epoch = info.sync_epoch;
                }
                Ok(FeedStep::Reset { reason, head })
            }
        }
    }
}
