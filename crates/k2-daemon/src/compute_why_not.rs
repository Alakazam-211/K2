//! Why an auto job waits where it is (a child of `compute_place`).
//!
//! - A shared node's own slots: [`Cand::node_slot_free`]. Every server's
//!   running jobs count against the node's `parallel_max`, and the offer
//!   says how many run (it is sent on every change). A quote rides the
//!   30 s offer and exists only for sizes this server asked there before,
//!   so a node full of other servers' jobs read as "starts now" when it had
//!   no quote for the size (the mover then sent the job into that node's
//!   queue) or when its quote was older than the jobs that filled it.
//! - "Not now" for each other granted node ([`not_now`]): the one reason
//!   it can't start the job now, in words. `k2 compute get` adds them to
//!   the job's `move` block, so a job that waits while another node looks
//!   free says which reason holds it (node full with other servers' jobs,
//!   this workspace's grant there, too little free memory, …).
//!
//! Only granted node names and node-wide numbers, as placement shows (S2).

use k2_core::compute::proto::refusal;
use k2_core::compute::share::{self, pending};
use k2_core::compute::store::JobRow;

use super::{candidates_report, mem_words, movable, skip_words, Cand, Want};

/// A node where every slot runs a job (any server's).
pub const NODE_FULL: &str = "node_full";
/// This server runs its share's most jobs there.
pub const SHARE_FULL: &str = "share_full";
/// This workspace runs its grant's most jobs there.
pub const WORKSPACE_FULL: &str = "workspace_running_full";
/// Not enough free CPU or memory there for the job's ask.
pub const NO_ROOM: &str = "no_room";
/// Its queue there says not now (the node's own reason, or a wait).
pub const NODE_QUEUE: &str = "node_queue";
/// Jobs already wait in that node's line (a solo node starts in order).
pub const QUEUED_THERE: &str = "queued_there";
/// It can't give the job's `--needs`.
pub const NEEDS: &str = "needs";
pub const OFFLINE: &str = "offline";
/// The node or this server's share there is paused, draining or locked.
pub const UNAVAILABLE: &str = "unavailable";
/// `--exclusive`: something runs there.
pub const NOT_IDLE: &str = "not_idle";
/// It can start the job now: the mover moves it on a coming pass.
pub const STARTS_NOW: &str = "starts_now";
/// None of the above, and still not now.
pub const NOT_NOW: &str = "not_now";

impl Cand {
    /// A slot free on the node itself: fewer jobs running there (every
    /// server's, plus asks sent since the offer) than its `parallel_max`.
    pub(crate) fn node_slot_free(&self) -> bool {
        self.offer.parallel_max == 0 || self.running_now() < self.offer.parallel_max
    }
}

/// One other node and why it can't start the job now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotNow {
    pub node: String,
    pub code: &'static str,
    pub words: String,
}

impl NotNow {
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({"node": self.node, "code": self.code, "words": self.words})
    }
}

fn cpus(m: u64) -> String {
    let s = format!("{:.1}", m as f64 / 1000.0);
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

/// "asks 4 CPUs and 16G; 2 CPUs and 32G free".
fn room_words(c: &Cand, w: &Want) -> String {
    let (mem, cpu) = c.free_now();
    let ask = match (w.cpu_millis, w.mem_on(&c.offer)) {
        (Some(cm), Some(m)) => format!("{} CPUs and {}", cpus(cm), mem_words(m)),
        (Some(cm), None) => format!("{} CPUs", cpus(cm)),
        (None, Some(m)) => mem_words(m),
        (None, None) => "an even share".to_string(),
    };
    format!("not enough free room: the job asks {ask}; {} CPUs and {} free", cpus(cpu), mem_words(mem))
}

/// Why `c` can't start `w` now, or `None` when it can (it can exactly
/// when [`Cand::starts_now`] says so and the mover may go there): the
/// first of the same checks that fails.
pub fn reason(c: &Cand, w: &Want) -> Option<(&'static str, String)> {
    if !c.online {
        return Some((OFFLINE, "offline".into()));
    }
    if !movable(c, w) {
        return Some((NEEDS, "can't give this job's --needs".into()));
    }
    if c.starts_now(w) {
        return None;
    }
    Some(first_reason(c, w).unwrap_or_else(|| (NOT_NOW, "can't start it now".into())))
}

fn first_reason(c: &Cand, w: &Want) -> Option<(&'static str, String)> {
    let running = c.running_now();
    let full = || (NODE_FULL, format!("full: it runs {running} of {} jobs (other servers' jobs count too)", c.offer.parallel_max));
    if let Some(v) = c.shared_view() {
        if !c.usable() {
            let node = format!("{:?}", c.offer.control).to_lowercase();
            return Some((UNAVAILABLE, format!("unavailable (this server's share there is {}, the node is {node})", v.state.as_str())));
        }
        if w.exclusive && running > 0 {
            return Some((NOT_IDLE, format!("--exclusive: {running} job(s) run there")));
        }
        if !c.node_slot_free() {
            return Some(full());
        }
        let ours = v.running + c.pending.jobs;
        if ours >= v.limits.max_running {
            return Some((SHARE_FULL, format!("this server runs {ours} of {} jobs there (its share)", v.limits.max_running)));
        }
        if c.ws_running(w) >= c.grant_cap() {
            return Some((WORKSPACE_FULL, format!("your workspace runs {} of {} there (its grant)", c.ws_running(w), c.grant_cap())));
        }
        if !c.fits_free(w) {
            return Some((NO_ROOM, room_words(c, w)));
        }
        if w.gpu.is_some_and(|g| c.offer.free.gpus < g.count) {
            return Some((NO_ROOM, "no free GPU".into()));
        }
        if !share::starts_now(v, w.cpu_millis, w.mem_bytes) {
            let q = share::quote_for(v, w.cpu_millis, w.mem_bytes);
            let why = q.and_then(|q| q.blocked_by.as_deref()).map(share::blocked_words);
            let eta = q.and_then(|q| q.eta_secs).map(|e| format!(", ETA {}", share::human_eta(e)));
            let words = match (why, eta) {
                (Some(b), e) => format!("its queue says not now: {b}{}", e.unwrap_or_default()),
                (None, Some(e)) => format!("its queue says not now{e}"),
                (None, None) if v.queued > 0 => format!("this server has {} job(s) waiting in its queue", v.queued),
                (None, None) => "its queue says not now".into(),
            };
            return Some((NODE_QUEUE, words));
        }
        return None;
    }
    if c.queued > 0 {
        return Some((QUEUED_THERE, format!("{} job(s) already wait there", c.queued)));
    }
    if !c.node_slot_free() {
        return Some(full());
    }
    let probe = refusal::JobAsk {
        workspace_id: &w.ws_id,
        cpu_millis: w.cpu_millis,
        mem_bytes: w.mem_on(&c.offer),
        disk_bytes: 0,
        exclusive: w.exclusive,
        needs_git: w.needs_git,
        gpus: w.gpu.map_or(0, |g| g.count),
    };
    if let Some(r) = refusal::offer_refusal(&c.offer, c.load.total() + c.pending.jobs, c.load.exclusive_running, &probe) {
        return Some((UNAVAILABLE, format!("{} ({})", r.message, r.code)));
    }
    if !c.fits_free(w) {
        return Some((NO_ROOM, room_words(c, w)));
    }
    if c.ws_running(w) >= c.grant_cap() {
        return Some((WORKSPACE_FULL, format!("your workspace runs {} of {} there (its grant)", c.ws_running(w), c.grant_cap())));
    }
    None
}

/// Each other node granted to the job's workspace that could take it, and
/// why it can't start it now (or that it can); then one line for the
/// granted nodes placement leaves out (offline, too small, …). Empty for
/// a pinned job or one already moving.
pub fn not_now(j: &JobRow, now: i64) -> (Vec<NotNow>, Option<String>) {
    if j.ctl.any.is_none() || j.reason.as_deref() == Some(pending::WITHDRAW) {
        return (Vec::new(), None);
    }
    let Some(plan) = j.plan() else { return (Vec::new(), None) };
    let w = Want::of_job(j, &plan);
    let report = candidates_report(&w, std::slice::from_ref(&j.node_id), now);
    let mut out: Vec<NotNow> = report
        .cands
        .iter()
        .map(|c| {
            let (code, words) = reason(c, &w).unwrap_or_else(|| (STARTS_NOW, "can start it now; K2 moves it on a coming pass".into()));
            NotNow { node: c.node.name.clone(), code, words }
        })
        .collect();
    out.sort_by(|a, b| a.node.cmp(&b.node));
    let total: u32 = report.skipped.values().sum();
    let left_out = (total > 0).then(|| report.skipped.iter().map(|(k, n)| skip_words(k, *n, &w)).collect::<Vec<_>>().join(", "));
    (out, left_out)
}

/// `k2 compute get`: add the other nodes' reasons to a waiting auto job's
/// `move` block (`notNow`, `leftOut`) and to its note, so the one line the
/// CLI prints says why it stays.
pub fn annotate(j: &JobRow, mv: &mut serde_json::Value, now: i64) {
    if mv.get("auto").and_then(|a| a.as_bool()) != Some(true) || mv.get("moving").and_then(|m| m.as_bool()) == Some(true) {
        return;
    }
    let (list, left_out) = not_now(j, now);
    let mut note = mv.get("note").and_then(|n| n.as_str()).unwrap_or_default().to_string();
    if list.is_empty() {
        note.push_str("; no other node of yours can take it");
    } else {
        let parts: Vec<String> = list.iter().map(|n| format!("{} ({})", n.node, n.words)).collect();
        note.push_str(&format!("; other nodes: {}", parts.join("; ")));
    }
    if let Some(l) = &left_out {
        note.push_str(&format!("; left out: {l}"));
    }
    mv["note"] = note.into();
    mv["notNow"] = list.iter().map(NotNow::json).collect::<Vec<_>>().into();
    mv["leftOut"] = left_out.into();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute_place::tests::{cand, shared, want};
    use k2_core::compute::proto::share as pshare;

    const GB: u64 = 1 << 30;

    #[test]
    fn a_shared_node_full_of_other_servers_jobs_never_starts_now() {
        // No quote for this size: the share alone said "starts now".
        let c = shared(cand("full", 30, 32, 2, 2), None);
        assert!(!c.starts_now(&want(2)), "2 of 2 slots run other servers' jobs");
        let (code, words) = reason(&c, &want(2)).expect("a reason");
        assert_eq!(code, NODE_FULL);
        assert!(words.contains("2 of 2"), "{words}");
        // A quote older than the jobs that filled the node says yes: the
        // offer's count wins.
        let stale = shared(cand("stale", 30, 32, 2, 2), Some((true, None)));
        assert!(!stale.starts_now(&want(2)));
        // One slot free: it starts.
        let room = shared(cand("room", 30, 32, 1, 2), None);
        assert!(room.starts_now(&want(2)));
        assert_eq!(reason(&room, &want(2)), None);
    }

    #[test]
    fn each_reason_names_its_limit() {
        // This workspace at its grant's cap there.
        let mut g = shared(cand("grant", 30, 32, 1, 3), None);
        g.load.running_by_workspace.insert(want(2).ws_id.clone(), 1);
        g.grant.as_mut().expect("grant").max_parallel = 1;
        let (code, words) = reason(&g, &want(2)).expect("a reason");
        assert_eq!((code, words.as_str()), (WORKSPACE_FULL, "your workspace runs 1 of 1 there (its grant)"));
        // Too little free memory.
        let small = shared(cand("small", 1, 32, 0, 2), None);
        let (code, words) = reason(&small, &want(4)).expect("a reason");
        assert_eq!(code, NO_ROOM);
        assert!(words.contains("asks 4G") && words.contains("1G free"), "{words}");
        // The quote says not now, with the node's own reason.
        let mut q = shared(cand("queue", 30, 32, 0, 2), None);
        q.offer.share.as_mut().expect("share").quotes =
            vec![pshare::Quote { cpu_millis: 0, mem_bytes: 2 * GB, starts_now: false, eta_secs: Some(600), eta_p90_secs: None, blocked_by: Some(pshare::blocked::PAUSED.into()) }];
        let (code, words) = reason(&q, &want(2)).expect("a reason");
        assert_eq!(code, NODE_QUEUE);
        assert!(words.contains("paused") && words.contains("ETA"), "{words}");
        // A solo node with jobs in line.
        let mut s = cand("solo", 30, 32, 0, 2);
        s.queued = 2;
        assert_eq!(reason(&s, &want(2)).map(|r| r.0), Some(QUEUED_THERE));
        // Offline.
        let mut o = cand("off", 30, 32, 0, 2);
        o.online = false;
        assert_eq!(reason(&o, &want(2)).map(|r| r.0), Some(OFFLINE));
    }
}
