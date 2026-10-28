//! Compute job tiers: the "airport" model (prd-compute-job-tiers-v1, v2 and
//! Rosson's 10-28 rules; protocol 5, `tiers2` under protocol 6).
//!
//! Tiers only order a queue. Nothing is reserved, and no tier ever stops,
//! pauses or pre-empts a running job (R2, R3). A node starts a job only
//! when its slots and memory both fit, whatever the line or turn.
//!
//! - **CI** takes the next free slot, ahead of everything queued. It needs
//!   the node owner's share `gate` flag for this server and, on the server,
//!   a grant with `allow_ci` (set by the owner or an Admin only).
//! - **Priority** (like TSA Pre) and **Standard** (`normal` on the wire,
//!   the default) are two lines. When both have jobs waiting, the node
//!   **alternates** starts between them, so Standard never starves. A
//!   Priority job at the head of the order still takes the next free slot.
//!   `--priority` needs a one-line `--why`; a workspace has at most
//!   [`PRIORITY_CAP`] Priority jobs queued or running (more run Standard).
//!   A guest server's Priority counts on a node only where the node's owner
//!   allows it (`queue.toml`), else it runs as Standard.
//! - **Batch**: no time window by default; it starts only when the node is
//!   idle (nothing running, nothing else ready), so at most one runs at a
//!   time. It never holds anyone. `--tier batch` implies `--detach` and a
//!   48 h queue timeout.
//! - **Turns**: inside each line, submitters take turns (workspaces first
//!   on a shared queue, then submitters, then first come). A job that has
//!   waited [`AGE_SECS`] or more skips the turns (it goes by arrival, ahead
//!   of its line's fresh jobs) but never jumps to another line, and the
//!   node's fit rules still apply.
//! - **"Next free slot"**: while a CI or Priority job heads the order and
//!   waits only for room, nothing ordered after it starts (hold 0). A
//!   Standard head keeps the shared queue's 20-minute backfill hold (none
//!   on a solo node). A Batch head never holds. A head refused for any
//!   other reason never holds (T3).
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

/// Offer feature (protocol 6, 0.46.6, prd-compute-job-tiers-v2): this node
/// knows [`Tier::Batch`], `Plan.submitter` and `Plan.submitter_max_running`,
/// alternates Priority and Standard, and takes turns per submitter. A
/// server writes `batch` and the submitter fields only to a node offering
/// it (a 0.46.5 parser rejects the variant); a node offers it only to a
/// server that said protocol ≥ [`crate::frames::PROTOCOL_TIERS2`], and
/// never shows such a server a Batch tier.
pub const FEATURE_TIERS2: &str = "tiers2";

/// A job waiting this long skips its line's turns (T5, v2 L4).
pub const AGE_SECS: i64 = 3600;

/// `--supersede KEY`: at most this many characters.
pub const SUPERSEDE_MAX: usize = 120;
/// `--submitter NAME`: at most this many characters.
pub const SUBMITTER_NAME_MAX: usize = 40;
/// `--why`: at most this many characters (one line).
pub const WHY_MAX: usize = 200;
/// At most this many Priority jobs per workspace, queued or running, on
/// this server (Rosson 10-28); more run Standard, with a notice.
pub const PRIORITY_CAP: usize = 2;
/// `--tier batch`: the default queue timeout (48 h covers any day).
pub const BATCH_QUEUE_TIMEOUT_SECS: i64 = 48 * 3600;

/// A job's tier. Ordered by rank: `Batch < Normal < Priority < Ci` (the
/// order says which line a tie goes to; Priority and Normal alternate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// 0.46.6 (`tiers2` only on the wire): idle time.
    Batch,
    /// Standard (the word users see); `normal` on the wire and in the DB.
    #[default]
    Normal,
    Priority,
    Ci,
}

impl Tier {
    pub const ALL: [Tier; 4] = [Tier::Ci, Tier::Priority, Tier::Normal, Tier::Batch];

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Batch => "batch",
            Tier::Normal => "normal",
            Tier::Priority => "priority",
            Tier::Ci => "ci",
        }
    }

    /// `standard` is accepted as Normal's user word.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "batch" => Some(Tier::Batch),
            "normal" | "standard" => Some(Tier::Normal),
            "priority" => Some(Tier::Priority),
            "ci" => Some(Tier::Ci),
            _ => None,
        }
    }

    /// The word people see: Standard for Normal.
    pub fn word(self) -> &'static str {
        match self {
            Tier::Batch => "Batch",
            Tier::Normal => "Standard",
            Tier::Priority => "Priority",
            Tier::Ci => "CI",
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
    /// set beside `Plan.tier`): CI is `gate`, Batch `batch` (a 0.46.4 node
    /// orders it below interactive work and ages it after 60 min),
    /// everything else `interactive` (T11: `--detach` no longer lowers a
    /// job).
    pub fn class(self) -> JobClass {
        match self {
            Tier::Ci => JobClass::Gate,
            Tier::Batch => JobClass::Batch,
            Tier::Priority | Tier::Normal => JobClass::Interactive,
        }
    }

    /// The tier a peer without `tiers2` may see: Batch is Normal there (a
    /// 0.46.5 parser rejects `batch`, T27).
    pub fn for_peer(self, tiers2: bool) -> Tier {
        if self == Tier::Batch && !tiers2 {
            Tier::Normal
        } else {
            self
        }
    }

    /// The short column tag: `CI`, `P`, `B`, or blank for Standard.
    pub fn tag(self) -> &'static str {
        match self {
            Tier::Ci => "CI",
            Tier::Priority => "P",
            Tier::Batch => "B",
            Tier::Normal => "",
        }
    }

    /// The line this tier stands in.
    pub fn line(self) -> Line {
        match self {
            Tier::Ci => Line::Ci,
            Tier::Priority => Line::Priority,
            Tier::Normal => Line::Standard,
            Tier::Batch => Line::Batch,
        }
    }
}

/// The line a queued job stands in (the airport model). CI goes first,
/// Priority and Standard alternate, Batch waits for an idle node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Line {
    Batch,
    Standard,
    Priority,
    Ci,
}

impl Line {
    /// "Next free slot": while a job of this line heads the order and
    /// waits only for room, nothing ordered after it starts (hold 0).
    pub fn holds(self) -> bool {
        matches!(self, Line::Priority | Line::Ci)
    }

    /// May a head of this line hold the queue after waiting long (the
    /// shared queue's 20-minute backfill hold)? Never Batch.
    pub fn backfill_holds(self) -> bool {
        self != Line::Batch
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Line::Batch => "batch",
            Line::Standard => "standard",
            Line::Priority => "priority",
            Line::Ci => "ci",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "batch" => Some(Line::Batch),
            "standard" => Some(Line::Standard),
            "priority" => Some(Line::Priority),
            "ci" => Some(Line::Ci),
            _ => None,
        }
    }
}

/// Which of Priority and Standard starts next when both have jobs waiting:
/// the one that didn't get the last start (Priority first with no history).
pub fn alternate_after(last: Option<Line>) -> Line {
    if last == Some(Line::Priority) {
        Line::Standard
    } else {
        Line::Priority
    }
}

/// Order lines for a start: CI, then Priority and Standard alternating
/// (after `last`, the line of the last Priority-or-Standard start), then
/// Batch. `has(line)`: some job waits in that line. The first line returned
/// with work is the one to pick from now.
pub fn next_line(has: impl Fn(Line) -> bool, last: Option<Line>) -> Option<Line> {
    if has(Line::Ci) {
        return Some(Line::Ci);
    }
    let first = alternate_after(last);
    let second = if first == Line::Priority { Line::Standard } else { Line::Priority };
    [first, second, Line::Batch].into_iter().find(|l| has(*l))
}

/// Has a job of `tier` that waited `waited_secs` aged past its line's
/// turns (`age_secs`; [`AGE_SECS`] in production)? CI goes by arrival
/// anyway.
pub fn aged(waited_secs: i64, age_secs: i64) -> bool {
    waited_secs >= age_secs
}

/// The tier a node or server counts for a job: CI only where the share
/// allows `gate` (`may_ci`), Priority only where Priority counts for this
/// server (`may_priority`); otherwise Normal (T7, T9). Batch stays Batch.
pub fn effective(tier: Tier, may_ci: bool, may_priority: bool) -> Tier {
    match tier {
        Tier::Ci if may_ci => Tier::Ci,
        Tier::Priority if may_priority => Tier::Priority,
        Tier::Batch => Tier::Batch,
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

/// `--why "<reason>"`: one line of 3–[`WHY_MAX`] printable characters.
pub fn valid_why(why: &str) -> bool {
    let t = why.trim();
    (3..=WHY_MAX).contains(&t.chars().count()) && !t.chars().any(char::is_control)
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
    /// `--priority` (or `--tier priority`) without `--why "<reason>"`.
    /// Exit 2.
    pub const PRIORITY_NEEDS_WHY: &str = "priority_needs_why";
    /// Notice: the workspace already has [`super::PRIORITY_CAP`] Priority
    /// jobs queued or running; this one runs Standard. A retier to Priority
    /// over the cap is refused with it (exit 3).
    pub const PRIORITY_CAP_REACHED: &str = "priority_cap_reached";
    /// A Batch job waits on the server for a node older than 0.46.6 to go
    /// idle, then goes as Standard (T28).
    pub const BATCH_IDLE_ONLY: &str = "batch_idle_only";
    /// Notice: no `--mem`: the job reserves the default and is stopped
    /// above it.
    pub const MEM_DEFAULTED: &str = "mem_defaulted";

    pub const ALL: &[&str] = &[
        CI_NOT_ALLOWED,
        REPLACED,
        TIER_HOLD,
        CRITICAL_PATH_TAKEN,
        PRIORITY_DOWNGRADED,
        NODE_TOO_OLD_FOR_TIERS,
        PRIORITY_NEEDS_WHY,
        PRIORITY_CAP_REACHED,
        BATCH_IDLE_ONLY,
        MEM_DEFAULTED,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_rank_ci_over_priority_over_normal_over_batch_and_map_to_old_classes() {
        assert!(Tier::Ci > Tier::Priority && Tier::Priority > Tier::Normal && Tier::Normal > Tier::Batch);
        for t in Tier::ALL {
            assert_eq!(Tier::parse(t.as_str()), Some(t));
            assert_eq!(serde_json::to_value(t).unwrap(), serde_json::Value::String(t.as_str().into()));
        }
        assert_eq!(Tier::parse("standard"), Some(Tier::Normal), "the user word");
        assert_eq!(Tier::parse("bulk"), None);
        assert_eq!(Tier::default(), Tier::Normal);
        assert_eq!(Tier::from_class(JobClass::Gate), Tier::Ci);
        assert_eq!(Tier::from_class(JobClass::Interactive), Tier::Normal);
        assert_eq!(Tier::from_class(JobClass::Batch), Tier::Normal, "old --detach jobs lose their demotion");
        assert_eq!(Tier::Ci.class(), JobClass::Gate);
        assert_eq!(Tier::Priority.class(), JobClass::Interactive, "an old node orders Priority as plain work");
        assert_eq!(Tier::Normal.class(), JobClass::Interactive, "a Normal --detach job is interactive (T11)");
        assert_eq!(Tier::Batch.class(), JobClass::Batch, "a 0.46.4 node demotes it");
        assert_eq!(Tier::Batch.for_peer(false), Tier::Normal, "never `batch` to a 0.46.5 peer (T27)");
        assert_eq!(Tier::Batch.for_peer(true), Tier::Batch);
        assert_eq!(Tier::Priority.for_peer(false), Tier::Priority);
        assert_eq!((Tier::Batch.tag(), Tier::Normal.word()), ("B", "Standard"));
    }

    /// The 0.46.5 enum (no Batch) parses every tier a `tiers`-only peer is
    /// sent (T27's golden).
    #[test]
    fn a_tiers_only_peer_never_sees_batch() {
        #[derive(Deserialize, Debug, PartialEq)]
        #[serde(rename_all = "snake_case")]
        enum Tier0465 {
            Normal,
            Priority,
            Ci,
        }
        for t in Tier::ALL {
            let wire = serde_json::to_value(t.for_peer(false)).unwrap();
            assert!(serde_json::from_value::<Tier0465>(wire).is_ok(), "{t:?}");
        }
        assert!(serde_json::from_value::<Tier0465>(serde_json::to_value(Tier::Batch).unwrap()).is_err(), "why the guard exists");
    }

    #[test]
    fn lines_alternate_priority_and_standard_after_ci_and_batch_waits() {
        let all = |_: Line| true;
        assert_eq!(next_line(all, None), Some(Line::Ci), "CI first, always");
        let no_ci = |l: Line| l != Line::Ci;
        assert_eq!(next_line(no_ci, None), Some(Line::Priority), "no history: Priority");
        assert_eq!(next_line(no_ci, Some(Line::Priority)), Some(Line::Standard), "after a Priority start, Standard");
        assert_eq!(next_line(no_ci, Some(Line::Standard)), Some(Line::Priority));
        let only_std = |l: Line| matches!(l, Line::Standard | Line::Batch);
        assert_eq!(next_line(only_std, Some(Line::Standard)), Some(Line::Standard), "one line waiting: it goes");
        assert_eq!(next_line(|l: Line| l == Line::Batch, Some(Line::Priority)), Some(Line::Batch), "Batch last");
        assert_eq!(next_line(|_: Line| false, None), None);
        assert!(Line::Ci.holds() && Line::Priority.holds() && !Line::Standard.holds() && !Line::Batch.holds());
        assert!(Line::Standard.backfill_holds() && !Line::Batch.backfill_holds());
        for l in [Line::Ci, Line::Priority, Line::Standard, Line::Batch] {
            assert_eq!(Line::parse(l.as_str()), Some(l));
        }
        assert!(aged(AGE_SECS, AGE_SECS) && !aged(AGE_SECS - 1, AGE_SECS));
    }

    #[test]
    fn effective_tier_needs_the_shares_word() {
        assert_eq!(effective(Tier::Ci, true, false), Tier::Ci);
        assert_eq!(effective(Tier::Ci, false, true), Tier::Normal, "no gate on the share: never CI");
        assert_eq!(effective(Tier::Priority, false, true), Tier::Priority);
        assert_eq!(effective(Tier::Priority, true, false), Tier::Normal, "a guest's Priority without the owner's word");
        assert_eq!(effective(Tier::Normal, true, true), Tier::Normal);
        assert_eq!(effective(Tier::Batch, false, false), Tier::Batch);
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
    fn supersede_keys_submitter_names_and_whys() {
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
        assert!(valid_why("release gate for 0.46.6"));
        assert!(!valid_why("  "));
        assert!(!valid_why("ok"));
        assert!(!valid_why("two\nlines"));
        assert!(!valid_why(&"w".repeat(WHY_MAX + 1)));
    }
}
