//! How many of this server's own nodes take a rollout's update at once
//! (0.46.13). A rollout used to offer every node its build at the same
//! moment: every node of a kind drained together, and jobs had nowhere to
//! go. Now, per OS and architecture, at most one node or a quarter of
//! them (the larger) update at a time, and while more than one is there,
//! at least one stays out of the update, taking work. The others wait
//! their turn (`pending`, code [`WAITING_TURN`]): they keep running and
//! taking jobs. The controller decides: nodes never update on their own,
//! so this works with every k2-node that takes remote updates.
//!
//! An explicit `--now` rollout (the owner stopping jobs on purpose) is not
//! staggered; `--batch N` still caps the whole rollout on top of this.

use std::collections::HashMap;

/// A pending node whose group is full: it updates when a slot frees.
pub const WAITING_TURN: &str = "waiting_turn";
/// A pending older node that would cut its running jobs short: it updates
/// once it's idle.
pub const WAITING_IDLE: &str = "waiting_idle";
/// The longest an older node waits to be idle before its update goes
/// anyway (its own 30 min drain, or a stop on a solo node, then applies).
pub const IDLE_WAIT_SECS: i64 = 12 * 3600;

/// Hold an older node's offer until it runs nothing? Only a k2-node whose
/// update would cut running jobs short (older than 0.46.13, jobs that
/// can't outlive it), while it runs some, for at most [`IDLE_WAIT_SECS`]
/// from the rollout's start.
pub fn wait_for_idle(cuts_jobs: bool, running: u32, waited: i64) -> bool {
    cuts_jobs && running > 0 && waited < IDLE_WAIT_SECS
}

/// The words on an older node waiting to be idle.
pub fn idle_detail(version: Option<&str>, running: u32) -> String {
    format!(
        "update staged, waiting for its {running} running job(s) to end: k2-node {} would stop them after 30 min (shared) or at once; it goes anyway after {} h",
        version.unwrap_or("?"),
        IDLE_WAIT_SECS / 3600
    )
}

/// The group a node updates in: its OS and architecture.
pub fn group(os: Option<&str>, arch: Option<&str>) -> String {
    format!("{}/{}", os.unwrap_or("?"), arch.unwrap_or("?"))
}

/// How many nodes of a group of `members` may update at once: one, or a
/// quarter of them rounded up, whichever is larger; with more than one,
/// never all of them.
pub fn cap(members: usize) -> usize {
    let c = members.div_ceil(4).max(1);
    if members > 1 {
        c.min(members - 1)
    } else {
        c
    }
}

/// One tick's count: each group's members (its online nodes and those
/// already updating) and how many are updating now.
#[derive(Debug, Default)]
pub struct Stagger {
    members: HashMap<String, usize>,
    updating: HashMap<String, usize>,
}

impl Stagger {
    /// A node of `group` that counts: online, or updating (it may be
    /// restarting).
    pub fn member(&mut self, group: &str) {
        *self.members.entry(group.to_string()).or_default() += 1;
    }

    /// A node of `group` already updating (offered, not settled).
    pub fn updating(&mut self, group: &str) {
        *self.updating.entry(group.to_string()).or_default() += 1;
    }

    /// May one more node of `group` start updating? Counts it if so.
    pub fn take(&mut self, group: &str) -> bool {
        let members = self.members.get(group).copied().unwrap_or(0).max(1);
        let busy = self.updating.entry(group.to_string()).or_default();
        if *busy >= cap(members) {
            return false;
        }
        *busy += 1;
        true
    }

    /// A slot [`Stagger::take`] gave that wasn't used (the offer failed).
    pub fn release(&mut self, group: &str) {
        if let Some(b) = self.updating.get_mut(group) {
            *b = b.saturating_sub(1);
        }
    }

    /// The words on a node waiting its turn.
    pub fn waiting_detail(&self, group: &str) -> String {
        let members = self.members.get(group).copied().unwrap_or(0).max(1);
        format!("update staged, waiting its turn: {} of {members} {group} node(s) update at a time; it keeps taking jobs meanwhile", cap(members))
    }
}

/// How long an offered node may report `staged` (draining) before its
/// place times out: the 35 min an older node's drain takes, or, when the
/// node said when its drain ends at the latest (`until`, 0.46.13), until
/// then plus the usual 10 min for the swap and health check.
pub fn staged_limit(offered_at: i64, drain_until: Option<i64>, older: i64, settle: i64) -> i64 {
    match drain_until {
        Some(u) => older.max(u.saturating_sub(offered_at).saturating_add(settle)),
        None => older,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_node_or_a_quarter_and_never_the_whole_group() {
        assert_eq!(cap(0), 1);
        assert_eq!(cap(1), 1, "a lone node still updates");
        assert_eq!(cap(2), 1, "one of two keeps working");
        assert_eq!(cap(4), 1);
        assert_eq!(cap(5), 2, "a quarter, rounded up");
        assert_eq!(cap(7), 2, "the Linux pool of seven: two at a time");
        assert_eq!(cap(8), 2);
        assert_eq!(cap(9), 3);
        assert_eq!(cap(40), 10);
    }

    /// Several nodes, mixed OS, one pool: each OS/arch group gets its own
    /// slots, and a group already updating takes no more.
    #[test]
    fn mixed_groups_take_turns_apart() {
        let linux = group(Some("linux"), Some("x86_64"));
        let mac = group(Some("macos"), Some("aarch64"));
        let mut s = Stagger::default();
        for _ in 0..7 {
            s.member(&linux);
        }
        for _ in 0..3 {
            s.member(&mac);
        }
        let linux_sent = (0..7).filter(|_| s.take(&linux)).count();
        let mac_sent = (0..3).filter(|_| s.take(&mac)).count();
        assert_eq!((linux_sent, mac_sent), (2, 1), "7 Linux nodes: 2 at once; 3 Macs: 1");
        assert!(!s.take(&linux) && !s.take(&mac), "full until a slot frees");
        assert!(s.waiting_detail(&linux).starts_with("update staged, waiting its turn: 2 of 7 linux/x86_64"), "{}", s.waiting_detail(&linux));

        // Next tick: one Linux node still updating (the other settled).
        let mut s = Stagger::default();
        for _ in 0..7 {
            s.member(&linux);
        }
        s.updating(&linux);
        assert!(s.take(&linux), "a freed slot goes to the next node");
        assert!(!s.take(&linux));

        // A lone node of its kind, and two of a kind.
        let arm = group(Some("linux"), Some("aarch64"));
        let mut s = Stagger::default();
        s.member(&arm);
        assert!(s.take(&arm) && !s.take(&arm));
        let mut s = Stagger::default();
        s.member(&arm);
        s.member(&arm);
        assert!(s.take(&arm), "one of two updates");
        assert!(!s.take(&arm), "the other keeps taking work");
        assert_eq!(group(None, None), "?/?");
    }

    /// An older node that would cut its jobs short waits until it's idle,
    /// at most 12 h; a node that keeps or waits for its jobs never does.
    #[test]
    fn an_older_node_that_cuts_jobs_waits_to_be_idle() {
        assert!(wait_for_idle(true, 1, 0));
        assert!(wait_for_idle(true, 3, IDLE_WAIT_SECS - 1));
        assert!(!wait_for_idle(true, 3, IDLE_WAIT_SECS), "it goes after 12 h");
        assert!(!wait_for_idle(true, 0, 0), "idle: it goes now");
        assert!(!wait_for_idle(false, 5, 0), "a node that keeps or waits for its jobs needs no wait");
        assert!(idle_detail(Some("0.46.12"), 2).contains("its 2 running job(s)"));
    }

    #[test]
    fn a_draining_node_has_until_its_drain_ends() {
        let (older, settle) = (35 * 60, 600);
        assert_eq!(staged_limit(1000, None, older, settle), older, "an older node: 35 min");
        assert_eq!(staged_limit(1000, Some(1000 + 4 * 3600 + 900), older, settle), 4 * 3600 + 900 + 600, "a 4 h job: its end, the grace, then the swap");
        assert_eq!(staged_limit(1000, Some(1100), older, settle), older, "never shorter than before");
    }
}
