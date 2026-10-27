//! Compute job tiers (prd-compute-job-tiers-v1 + its vs-live T1–T22;
//! protocol 5, 0.46.5): **CI**, then **Priority** (the critical path), then
//! **Normal**.
//!
//! Tiers only order a queue. Nothing is reserved, and no tier ever stops,
//! pauses or pre-empts a running job (R2, R3).
//!
//! - **CI** takes the next free slot, ahead of everything queued. It needs
//!   the node owner's share `gate` flag for this server and, on the server,
//!   a grant with `allow_ci` (set by the owner or an Admin only).
//! - **Priority** takes the next free slot, never ahead of CI. One
//!   submitter per server at a time is a **courtesy, not enforced**, and
//!   Priority is optional: when no result blocks anyone, leave everything
//!   Normal. A guest server's Priority counts on a node only where the
//!   node's owner allows it (`queue.toml`), else it runs as Normal.
//! - **Normal** is the default: strict first come, first served inside a
//!   workspace, workspaces (and on a shared node, servers and sessions)
//!   take turns as before.
//! - **Aging** (T5): a Normal job waiting [`AGE_SECS`] ranks above fresh
//!   Normal jobs but below Priority, and never above CI. It keeps Normal's
//!   hold rule.
//! - **"Next free slot"**: while a CI or Priority job heads the order and
//!   waits only for room, nothing ordered after it starts (hold 0). A
//!   Normal head keeps the shared queue's 20-minute backfill hold (none on
//!   a solo node). A head refused for any other reason never holds (T3).
//!
//! The rules here are shared by the three orderers (T2), each of which
//! keeps its own structure: the node queue ([`crate::queue`], shared
//! nodes), the server's solo scheduler (`k2-core` `compute::scheduler`),
//! and the server's send order into a shared node (`k2-daemon`
//! `compute_ws::rotation_order`).

use serde::{Deserialize, Serialize};

use crate::refusal;
use crate::share::JobClass;

/// Offer feature (protocol 5): this node knows `Plan.tier`, keeps the tier
/// in its ledger, orders its queue by tier, and takes the `priority` key in
/// `servers.limits`. The server sends `Plan.tier` and the key only to nodes
/// offering it; a node offers it only to servers that said protocol ≥ 5.
pub const FEATURE_TIERS: &str = "tiers";

/// A Normal job waiting this long ranks above fresh Normal work (T5).
pub const AGE_SECS: i64 = 3600;

/// `--supersede KEY`: at most this many characters.
pub const SUPERSEDE_MAX: usize = 120;
/// `--submitter NAME`: at most this many characters.
pub const SUBMITTER_NAME_MAX: usize = 40;

/// A job's tier. Ordered by rank: `Normal < Priority < Ci`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    #[default]
    Normal,
    Priority,
    Ci,
}

impl Tier {
    pub const ALL: [Tier; 3] = [Tier::Ci, Tier::Priority, Tier::Normal];

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Normal => "normal",
            Tier::Priority => "priority",
            Tier::Ci => "ci",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "normal" => Some(Tier::Normal),
            "priority" => Some(Tier::Priority),
            "ci" => Some(Tier::Ci),
            _ => None,
        }
    }

    /// A job from a peer that knows only classes (protocol ≤ 4): `gate` is
    /// CI, `interactive` and `batch` are Normal (old `--detach` jobs lose
    /// their demotion; prd §3.2, G4).
    pub fn from_class(c: JobClass) -> Self {
        match c {
            JobClass::Gate => Tier::Ci,
            JobClass::Interactive | JobClass::Batch => Tier::Normal,
        }
    }

    /// The class an old peer orders this tier by (`Plan.class` is always
    /// set beside `Plan.tier`): CI is `gate`, everything else
    /// `interactive` (T11: `--detach` no longer lowers a job).
    pub fn class(self) -> JobClass {
        match self {
            Tier::Ci => JobClass::Gate,
            Tier::Priority | Tier::Normal => JobClass::Interactive,
        }
    }

    /// The short column tag: `CI`, `P`, or blank for Normal.
    pub fn tag(self) -> &'static str {
        match self {
            Tier::Ci => "CI",
            Tier::Priority => "P",
            Tier::Normal => "",
        }
    }
}

/// Where a queued job ranks right now: its tier, with aged Normal work
/// between Priority and fresh Normal (T5). Ordered by rank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Band {
    Normal,
    /// A Normal job that waited [`AGE_SECS`] or more.
    Aged,
    Priority,
    Ci,
}

impl Band {
    /// "Next free slot": while a job of this band heads the order and
    /// waits only for room, nothing ordered after it starts (hold 0). Aged
    /// Normal keeps Normal's hold rule (T5).
    pub fn holds(self) -> bool {
        matches!(self, Band::Priority | Band::Ci)
    }

    /// Strictly by arrival, with no turns between workspaces or sessions:
    /// CI, Priority and aged Normal (rare and urgent, or already old).
    pub fn by_arrival(self) -> bool {
        self != Band::Normal
    }
}

/// The band of a job of `tier` that has waited `waited_secs` (aging after
/// `age_secs`; [`AGE_SECS`] in production).
pub fn band(tier: Tier, waited_secs: i64, age_secs: i64) -> Band {
    match tier {
        Tier::Ci => Band::Ci,
        Tier::Priority => Band::Priority,
        Tier::Normal if waited_secs >= age_secs => Band::Aged,
        Tier::Normal => Band::Normal,
    }
}

/// The tier a node or server counts for a job: CI only where the share
/// allows `gate` (`may_ci`), Priority only where Priority counts for this
/// server (`may_priority`); otherwise Normal (T7, T9).
pub fn effective(tier: Tier, may_ci: bool, may_priority: bool) -> Tier {
    match tier {
        Tier::Ci if may_ci => Tier::Ci,
        Tier::Priority if may_priority => Tier::Priority,
        Tier::Ci | Tier::Priority | Tier::Normal => Tier::Normal,
    }
}

/// Solo hold rule (T3): a CI or Priority head that isn't placed holds every
/// later start only when it waits for room on the node. Any other refusal
/// (`grant_parallel_full`, `workspace_refused`, `tool_missing`, a per-job
/// `disk_low`, a node-wide stop) never holds: holding on those could stop
/// the node for good.
pub fn waits_for_room(code: &str) -> bool {
    matches!(code, refusal::PARALLEL_FULL | refusal::LOCAL_RESOURCE_LIMIT | refusal::WAITING_FOR_IDLE | refusal::EXCLUSIVE_RUNNING)
}

/// `--supersede KEY`: 1–[`SUPERSEDE_MAX`] printable characters, no
/// control characters.
pub fn valid_supersede(key: &str) -> bool {
    (1..=SUPERSEDE_MAX).contains(&key.chars().count()) && key.chars().all(|c| !c.is_control()) && key.trim() == key
}

/// `--submitter NAME`: 1–[`SUBMITTER_NAME_MAX`] of `a-z 0-9 . _ -`.
pub fn valid_submitter_name(name: &str) -> bool {
    (1..=SUBMITTER_NAME_MAX).contains(&name.len()) && name.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c))
}

/// Codes new with tiers (refusals, notices, a cancel reason).
pub mod codes {
    /// `--tier ci` (or `--gate`) without a grant with `allow_ci`, or on a
    /// share without `gate`; also `--tier priority --exclusive` without
    /// `allow_ci` (T18). Exit 3.
    pub const CI_NOT_ALLOWED: &str = "ci_not_allowed";
    /// A queued job ends `cancelled` with this reason when a newer job with
    /// the same `--supersede` key replaced it (only ever a queued job; the
    /// node's own `superseded` means something else, V47).
    pub const REPLACED: &str = "replaced";
    /// The solo scheduler's wait reason behind a CI or Priority head that
    /// takes the next free slot.
    pub const TIER_HOLD: &str = "tier_hold";
    /// Notice: another submitter on this server already has Priority work
    /// queued or running (a courtesy note; the job still submits).
    pub const CRITICAL_PATH_TAKEN: &str = "critical_path_taken";
    /// Notice: this server's Priority doesn't count on that node (a guest
    /// share without `priority`); it runs as Normal.
    pub const PRIORITY_DOWNGRADED: &str = "priority_downgraded";
    /// Notice: that node runs an older k2-node without tiers; Priority has
    /// no effect there (CI still runs as `gate`).
    pub const NODE_TOO_OLD_FOR_TIERS: &str = "node_too_old_for_tiers";

    pub const ALL: &[&str] = &[CI_NOT_ALLOWED, REPLACED, TIER_HOLD, CRITICAL_PATH_TAKEN, PRIORITY_DOWNGRADED, NODE_TOO_OLD_FOR_TIERS];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_rank_ci_over_priority_over_normal_and_map_to_old_classes() {
        assert!(Tier::Ci > Tier::Priority && Tier::Priority > Tier::Normal);
        for t in Tier::ALL {
            assert_eq!(Tier::parse(t.as_str()), Some(t));
            assert_eq!(serde_json::to_value(t).unwrap(), serde_json::Value::String(t.as_str().into()));
        }
        assert_eq!(Tier::parse("batch"), None, "Batch is 0.46.6");
        assert_eq!(Tier::default(), Tier::Normal);
        assert_eq!(Tier::from_class(JobClass::Gate), Tier::Ci);
        assert_eq!(Tier::from_class(JobClass::Interactive), Tier::Normal);
        assert_eq!(Tier::from_class(JobClass::Batch), Tier::Normal, "old --detach jobs lose their demotion");
        assert_eq!(Tier::Ci.class(), JobClass::Gate);
        assert_eq!(Tier::Priority.class(), JobClass::Interactive, "an old node orders Priority as plain work");
        assert_eq!(Tier::Normal.class(), JobClass::Interactive, "a Normal --detach job is interactive (T11)");
    }

    #[test]
    fn aged_normal_ranks_between_priority_and_fresh_normal_and_keeps_normals_hold() {
        assert_eq!(band(Tier::Normal, AGE_SECS - 1, AGE_SECS), Band::Normal);
        assert_eq!(band(Tier::Normal, AGE_SECS, AGE_SECS), Band::Aged);
        assert_eq!(band(Tier::Priority, 0, AGE_SECS), Band::Priority);
        assert_eq!(band(Tier::Priority, 99 * AGE_SECS, AGE_SECS), Band::Priority);
        assert_eq!(band(Tier::Ci, 0, AGE_SECS), Band::Ci);
        assert!(Band::Ci > Band::Priority && Band::Priority > Band::Aged && Band::Aged > Band::Normal);
        assert!(Band::Ci.holds() && Band::Priority.holds());
        assert!(!Band::Aged.holds(), "an old Normal job never drains a node (T5)");
        assert!(!Band::Normal.holds());
        assert!(Band::Aged.by_arrival() && !Band::Normal.by_arrival());
    }

    #[test]
    fn effective_tier_needs_the_shares_word() {
        assert_eq!(effective(Tier::Ci, true, false), Tier::Ci);
        assert_eq!(effective(Tier::Ci, false, true), Tier::Normal, "no gate on the share: never CI");
        assert_eq!(effective(Tier::Priority, false, true), Tier::Priority);
        assert_eq!(effective(Tier::Priority, true, false), Tier::Normal, "a guest's Priority without the owner's word");
        assert_eq!(effective(Tier::Normal, true, true), Tier::Normal);
    }

    #[test]
    fn only_room_refusals_hold_a_solo_node() {
        for c in [refusal::PARALLEL_FULL, refusal::LOCAL_RESOURCE_LIMIT, refusal::WAITING_FOR_IDLE, refusal::EXCLUSIVE_RUNNING] {
            assert!(waits_for_room(c), "{c}");
        }
        for c in [refusal::WORKSPACE_REFUSED, refusal::TOOL_MISSING, refusal::DISK_LOW, refusal::LOCAL_PAUSED, refusal::FOREIGN_LOCK, "grant_parallel_full", "node_offline"] {
            assert!(!waits_for_room(c), "{c} must never hold the node");
        }
    }

    #[test]
    fn supersede_keys_and_submitter_names() {
        assert!(valid_supersede("ci/main/rust"));
        assert!(!valid_supersede(""));
        assert!(!valid_supersede(" lead"));
        assert!(!valid_supersede("a\nb"));
        assert!(valid_supersede(&"k".repeat(SUPERSEDE_MAX)));
        assert!(!valid_supersede(&"k".repeat(SUPERSEDE_MAX + 1)));
        assert!(valid_submitter_name("builder-ptyd.2_a"));
        assert!(!valid_submitter_name("Builder"));
        assert!(!valid_submitter_name("a/b"));
        assert!(!valid_submitter_name(""));
        assert!(!valid_submitter_name(&"a".repeat(SUBMITTER_NAME_MAX + 1)));
    }
}
