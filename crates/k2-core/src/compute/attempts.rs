//! A job's attempts: one record per generation that ended without ending
//! the job. K2 starts a new generation on its own when an attempt ends in
//! an infrastructure failure and the job has retries left (a node update,
//! a lost node, a sync or spawn failure), when a shared node gives back a
//! job that hadn't started yet (a grant change re-sends it, a mover takes
//! it elsewhere), and when a node turns out not to have it.
//!
//! The record keeps what `k2 compute get` needs to say what happened (the
//! node, start and end, the end and why, what K2 did next) and what lets
//! the attempt's output stay readable: its log is the controller's
//! per-generation file ([`super::logs::path`]), its `--artifact` files
//! stay on its node under that generation (7 days), and the list of them
//! is kept here. It rides in the job's controller options
//! ([`JobCtl::attempts`]), so no migration; it is bounded
//! ([`MAX_RECORDS`]).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::proto::results::Outputs;
use super::store::{JobCtl, JobRow};
use super::{logs, message};
use crate::brand::CLI_NAME;

/// Records kept per job. Past it, the oldest generation that never
/// started goes first (it has no log and no files).
pub const MAX_RECORDS: usize = 24;

/// How long the controller keeps a log file (`logs::sweep`), in days.
pub const LOG_KEEP_DAYS: u64 = 14;

/// One generation that ended without ending the job.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptRecord {
    pub attempt: i64,
    pub generation: i64,
    pub node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    pub ended_at: i64,
    /// The row's state and reason when it ended (`interrupted` +
    /// `node_update`). A generation that ended while K2 waited for the
    /// node's answer to an ask keeps the outcome here (`assigned` +
    /// `withdrawn`, `withdraw_unanswered`, `node_lost`, `resent`), never
    /// the pending marker; the marker is in `asked`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The pending ask (`withdraw_requested`, `withdraw_resend`,
    /// `pull_requested`, …) the node was answering when it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    /// What K2 did next: one of [`next`].
    pub next: String,
    /// The last log sequence number this server stored for it.
    #[serde(default)]
    pub log_seq: i64,
    /// What the node reported it kept (`--artifact` files, tests, notes).
    /// None: nothing reported (yet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Outputs>,
}

/// What K2 did with a generation that ended without ending the job (the
/// `reason` the requeue writes).
pub mod next {
    /// An infrastructure end, retried on its own (`--retries`).
    pub const RETRY_AUTO: &str = "retry_backoff";
    /// `k2 compute retry`.
    pub const RETRY_ASKED: &str = "retry";
    /// A grant change (or a tier change) re-sent it to the same node.
    pub const RESENT: &str = "resent";
    /// A shared node gave it back so it could move.
    pub const WITHDRAWN: &str = "withdrawn";
    /// Its node went offline before it started there.
    pub const NODE_LOST: &str = "node_lost";
    /// Its node no longer had it in its queue.
    pub const NOT_ON_NODE: &str = "not_on_node";
    /// `k2 compute move` took it back from a shared node's queue.
    pub const PULLED_BACK: &str = "pulled_back";
    /// Its node never answered a request to give it back
    /// ([`super::super::share::ASK_SETTLE_SECS`]): K2 took it back itself
    /// and told the node not to run it.
    pub const WITHDRAW_UNANSWERED: &str = "withdraw_unanswered";
}

/// The record of `j`'s current generation, as it ends now.
pub fn record_of(j: &JobRow, next: &str, now: i64) -> AttemptRecord {
    // A pending ask is never an end: the record says what came of it.
    let asked = j.reason.clone().filter(|r| super::share::pending::any(Some(r)));
    let reason = if asked.is_some() { Some(next.to_string()) } else { j.reason.clone() };
    AttemptRecord {
        attempt: j.attempt,
        generation: j.generation,
        node_id: j.node_id.clone(),
        assigned_at: j.assigned_at,
        started_at: j.started_at,
        ended_at: j.ended_at.unwrap_or(now),
        state: j.state.clone(),
        reason,
        asked,
        detail: j.detail.clone(),
        exit_code: j.exit_code,
        next: next.to_string(),
        log_seq: logs::last_seq(&j.id, j.generation) as i64,
        outputs: super::result::outputs(j).map(slim),
    }
}

/// Only what a later fetch and `get` need (no tool versions, resources).
fn slim(o: Outputs) -> Outputs {
    Outputs { artifacts: o.artifacts, tests: o.tests, notes: o.notes, ..Default::default() }
}

/// Add a record (one per generation), within [`MAX_RECORDS`].
pub fn push(ctl: &mut JobCtl, rec: AttemptRecord) {
    ctl.attempts.retain(|r| r.generation != rec.generation);
    ctl.attempts.push(rec);
    ctl.attempts.sort_by_key(|r| r.generation);
    while ctl.attempts.len() > MAX_RECORDS {
        let i = ctl.attempts.iter().position(|r| r.started_at.is_none()).unwrap_or(0);
        ctl.attempts.remove(i);
    }
}

/// A job's options read before a requeue and written back after it must
/// not drop the records the requeue added: keep the stored list when it
/// knows a later generation (or more reported files).
pub fn keep_newer(stored: &JobCtl, ctl: &mut JobCtl) {
    let key = |c: &JobCtl| (c.attempts.iter().map(|r| r.generation).max().unwrap_or(0), c.attempts.iter().filter(|r| r.outputs.is_some()).count());
    if key(stored) > key(ctl) {
        ctl.attempts = stored.attempts.clone();
    }
}

/// A node reported an earlier attempt's files after this server had
/// already ended that attempt (the node was offline then): keep them on
/// its record. True when a record took them.
pub fn attach_outputs(ctl: &mut JobCtl, node_id: &str, generation: i64, o: Outputs) -> bool {
    match ctl.attempts.iter_mut().find(|r| r.generation == generation && r.node_id == node_id) {
        Some(r) if r.outputs.is_none() => {
            r.outputs = Some(slim(o));
            true
        }
        _ => false,
    }
}

/// How an attempt ended, in plain words (`node` = its node's name).
pub fn end_words(state: &str, reason: Option<&str>, started: bool, node: &str) -> String {
    let terminal = matches!(state, "done" | "failed" | "cancelled" | "timeout" | "interrupted" | "unknown");
    if !terminal {
        if reason == Some(next::WITHDRAW_UNANSWERED) {
            return format!("{node} didn't answer a request to give it back within {}", ask_settle_words());
        }
        return if started { format!("it was running on {node}") } else { format!("it never started on {node}") };
    }
    let r = reason.unwrap_or("");
    match r {
        "node_update" => format!("{node} stopped it while updating or restarting k2-node (an update waits for running jobs only up to its drain limit)"),
        "node_restart" => format!("{node}'s k2-node restarted and couldn't keep the job running"),
        "node_reboot" => format!("{node} rebooted"),
        "node_shutdown" => format!("{node} shut down"),
        "node_lost" => format!("{node} went offline and stayed offline too long while it ran"),
        "not_on_node" => format!("{node} had no record of the job (the start never reached it, or its record was lost)"),
        "node_removed" => format!("{node} was removed from this server"),
        "sync_failed" => format!("{node} couldn't get the job's source code"),
        "spawn_failed" => format!("{node} couldn't start the command"),
        "job_dir_failed" | "wipe_failed" => format!("{node} couldn't prepare the job's folder"),
        "package_not_approved" | "package_missing" | "package_install_failed" => format!("a package the job needs isn't available on {node}"),
        "tool_install_failed" => format!("a tool the job needs (--needs) wouldn't install on {node}"),
        "relay_cap" => format!("the job's source was too big to send to {node} over the relay"),
        "superseded" => format!("{node} was told to stop it (this server had already moved on)"),
        "stalled" => "it printed nothing for its --stall-timeout".to_string(),
        "timeout" => "it ran past its --timeout".to_string(),
        "mem_cap" => "it used more memory than its --mem".to_string(),
        "disk_budget" => format!("its build output grew past {node}'s whole disk budget"),
        _ => match state {
            "unknown" => format!("{node} lost track of it"),
            "interrupted" => format!("{node} interrupted it{}", if r.is_empty() { String::new() } else { format!(" ({r})") }),
            "failed" => format!("an infrastructure error on {node}{}", if r.is_empty() { String::new() } else { format!(" ({r})") }),
            "done" => "its command finished".to_string(),
            "cancelled" => "it was cancelled".to_string(),
            other => other.to_string(),
        },
    }
}

/// What K2 did next, in plain words. `then` = the node the next
/// generation went to, when it differs.
pub fn next_words(next: &str, node: &str, then: Option<&str>) -> String {
    let elsewhere = then.map(|t| format!(" on {t}")).unwrap_or_default();
    match next {
        next::RETRY_AUTO => format!("retried automatically{elsewhere} (an infrastructure end, not your command's result; --retries 0 turns this off)"),
        next::RETRY_ASKED => format!("retried on request ({CLI_NAME} compute retry){elsewhere}"),
        next::RESENT => format!("taken back from {node}'s queue before it started and sent again with the grant's new caps (no retry used)"),
        next::WITHDRAWN => format!("taken back from {node}'s queue before it started, to move it{} (no retry used)", then.map(|t| format!(" to {t}")).unwrap_or_default()),
        next::NODE_LOST => format!("{node} went offline before it started it; placed again{elsewhere} (no retry used)"),
        next::NOT_ON_NODE => format!("{node} no longer had it in its queue; placed again{elsewhere} (no retry used)"),
        next::PULLED_BACK => format!("taken back from {node}'s queue as asked ({CLI_NAME} compute move); placed again{elsewhere} (no retry used)"),
        next::WITHDRAW_UNANSWERED => format!(
            "{node} didn't answer a request to give it back within {}; {k2} took it back ({node} is told not to run it) and placed it again{elsewhere} (no retry used)",
            ask_settle_words(),
            k2 = crate::brand::PRODUCT_NAME
        ),
        other => format!("placed again{elsewhere} ({other})"),
    }
}

/// A move in a few words, for a follower's `moved:` line (`None`: not a
/// move, a retry). `then` = the node the next generation went to.
pub fn move_words(next: &str, node: &str, then: Option<&str>) -> Option<String> {
    let to = then.map(|t| format!(" for {t}")).unwrap_or_default();
    Some(match next {
        next::WITHDRAWN => format!("{node} gave it back from its queue before it started{to}"),
        next::RESENT => format!("{node} gave it back from its queue to take the grant's new caps"),
        next::NODE_LOST => format!("{node} went offline before it started it"),
        next::NOT_ON_NODE => format!("{node} no longer had it in its queue"),
        next::PULLED_BACK => format!("taken back from {node}'s queue as asked ({CLI_NAME} compute move)"),
        next::WITHDRAW_UNANSWERED => format!("{node} didn't answer a request to give it back within {}; {k2} took it back ({node} is told not to run it)", ask_settle_words(), k2 = crate::brand::PRODUCT_NAME),
        _ => return None,
    })
}

/// "3 min" ([`super::share::ASK_SETTLE_SECS`]).
pub fn ask_settle_words() -> String {
    format!("{} min", super::share::ASK_SETTLE_SECS / 60)
}

/// The `--artifact` side of one attempt for `get`: `not_asked` (no
/// `--artifact`), `never_started`, `kept` (files listed), `none_matched`,
/// `pending` (the attempt is still running), `not_collected` (it ended
/// and its node never reported its files).
fn artifacts_json(asked: bool, started: bool, ended: bool, outputs: Option<&Outputs>, reason: Option<&str>, node: &str, fetch: &str) -> serde_json::Value {
    let (status, note) = match outputs {
        _ if !started && ended => ("never_started", None),
        _ if !asked => ("not_asked", None),
        Some(o) if !o.artifacts.is_empty() => ("kept", Some(format!("on {node} for 7 days: {fetch}"))),
        Some(_) => ("none_matched", Some("the --artifact globs matched nothing".to_string())),
        None if !ended => ("pending", Some("collected when the attempt ends".to_string())),
        None if reason == Some("node_lost") => (
            "not_collected",
            Some(format!("{node} was offline when this attempt ended, so its --artifact files weren't collected; if {node} comes back within 7 days and reports them, they show here")),
        ),
        None => ("not_collected", Some(format!("{node} reported this attempt's end without its --artifact files"))),
    };
    let files = outputs.map(|o| o.artifacts.clone()).unwrap_or_default();
    serde_json::json!({"status": status, "files": files, "note": note})
}

fn log_json(job_id: &str, generation: i64, cmd: &str) -> serde_json::Value {
    let bytes = std::fs::metadata(logs::path(job_id, generation)).map(|m| m.len()).ok();
    serde_json::json!({
        "available": bytes.is_some_and(|b| b > 0),
        "bytes": bytes.unwrap_or(0),
        "command": cmd,
        "keptDays": LOG_KEEP_DAYS,
    })
}

/// Every attempt of `j`, oldest first, the current one last (for `get`,
/// `get --json` and `logs`). `names`: node id → name.
pub fn list_json(j: &JobRow, names: &HashMap<String, String>) -> Vec<serde_json::Value> {
    let short = message::short(&j.id);
    let name = |id: &str| names.get(id).cloned().unwrap_or_else(|| id.to_string());
    let asked = j.plan().is_some_and(|p| !p.artifacts.is_empty());
    let recs = &j.ctl.attempts;
    // The `--attempt N` a record answers to: the last generation of N that
    // ran (or the last of N).
    let picked: Vec<i64> = recs.iter().filter_map(|r| pick_record(j, r.attempt).map(|p| p.generation)).collect();
    let mut out = Vec::with_capacity(recs.len() + 1);
    for (i, r) in recs.iter().enumerate() {
        let node = name(&r.node_id);
        let then_id = recs.get(i + 1).map(|n| n.node_id.as_str()).unwrap_or(j.node_id.as_str());
        let then = (then_id != r.node_id).then(|| name(then_id));
        let addressable = picked.contains(&r.generation);
        let logs_cmd = if addressable { format!("{CLI_NAME} compute logs {short} --attempt {}", r.attempt) } else { format!("(generation {}: never started, no log)", r.generation) };
        let fetch = format!("{CLI_NAME} compute fetch {short} --attempt {}", r.attempt);
        out.push(serde_json::json!({
            "attempt": r.attempt,
            "generation": r.generation,
            "nodeId": r.node_id,
            "node": node,
            "current": false,
            "assignedAt": r.assigned_at,
            "startedAt": r.started_at,
            "endedAt": r.ended_at,
            "state": r.state,
            "reason": r.reason,
            "asked": r.asked,
            "detail": r.detail,
            "exitCode": r.exit_code,
            "why": end_words(&r.state, r.reason.as_deref(), r.started_at.is_some(), &node),
            "next": next_words(&r.next, &node, then.as_deref()),
            "nextCode": r.next,
            // A move (no retry used) in a few words; null for a retry.
            "moved": move_words(&r.next, &node, then.as_deref()),
            "log": log_json(&j.id, r.generation, &logs_cmd),
            "artifacts": artifacts_json(asked, r.started_at.is_some(), true, r.outputs.as_ref(), r.reason.as_deref(), &node, &fetch),
        }));
    }
    let node = name(&j.node_id);
    let ended = j.is_terminal();
    let state = super::result::shown_state(j);
    let outputs = super::result::outputs(j);
    out.push(serde_json::json!({
        "attempt": j.attempt,
        "generation": j.generation,
        "nodeId": j.node_id,
        "node": node,
        "current": true,
        "assignedAt": j.assigned_at,
        "startedAt": j.started_at,
        "endedAt": j.ended_at,
        "state": state,
        "reason": j.reason,
        "detail": j.detail,
        "exitCode": j.exit_code,
        "why": ended.then(|| end_words(&j.state, j.reason.as_deref(), j.started_at.is_some(), &node)),
        "next": serde_json::Value::Null,
        "log": log_json(&j.id, j.generation, &format!("{CLI_NAME} compute logs {short}")),
        "artifacts": artifacts_json(asked, j.started_at.is_some(), ended, outputs.as_ref(), j.reason.as_deref(), &node, &format!("{CLI_NAME} compute fetch {short}")),
    }));
    out
}

/// One attempt to read: its generation, node and reported files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub attempt: i64,
    pub generation: i64,
    pub node_id: String,
    pub state: String,
    pub outputs: Option<Outputs>,
    pub current: bool,
}

/// `--attempt N`: the current attempt (also when `n` is None), or the
/// last generation of attempt N that ran (a generation that never
/// started has no log or files), else the last generation of N.
pub fn pick(j: &JobRow, n: Option<i64>) -> Option<Pick> {
    match n {
        None => Some(current(j)),
        Some(n) if n == j.attempt => Some(current(j)),
        Some(n) => pick_record(j, n).map(|r| Pick { attempt: r.attempt, generation: r.generation, node_id: r.node_id.clone(), state: r.state.clone(), outputs: r.outputs.clone(), current: false }),
    }
}

fn current(j: &JobRow) -> Pick {
    Pick { attempt: j.attempt, generation: j.generation, node_id: j.node_id.clone(), state: j.state.clone(), outputs: super::result::outputs(j), current: true }
}

fn pick_record(j: &JobRow, n: i64) -> Option<&AttemptRecord> {
    let of_n: Vec<&AttemptRecord> = j.ctl.attempts.iter().filter(|r| r.attempt == n).collect();
    of_n.iter().rev().find(|r| r.started_at.is_some() || r.log_seq > 0).or(of_n.last()).copied()
}

/// The attempts a job has, for a refusal: `attempts 1-3`.
pub fn range_words(j: &JobRow) -> String {
    let first = j.ctl.attempts.iter().map(|r| r.attempt).min().unwrap_or(j.attempt).min(j.attempt);
    if first == j.attempt {
        format!("attempt {}", j.attempt)
    } else {
        format!("attempts {first}-{}", j.attempt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::proto::results::ArtifactInfo;

    fn row(attempt: i64, generation: i64) -> JobRow {
        JobRow { id: "jobid-0001".into(), node_id: "n1".into(), state: "running".into(), attempt, generation, ..Default::default() }
    }

    fn rec(attempt: i64, generation: i64, started: bool, next: &str) -> AttemptRecord {
        AttemptRecord {
            attempt,
            generation,
            node_id: "n1".into(),
            started_at: started.then_some(10),
            ended_at: 20,
            state: if started { "interrupted".into() } else { "assigned".into() },
            reason: Some(if started { "node_update".into() } else { "withdraw_resend".into() }),
            next: next.into(),
            ..Default::default()
        }
    }

    #[test]
    fn pick_reads_the_generation_of_an_attempt_that_ran() {
        let mut j = row(2, 3);
        push(&mut j.ctl, rec(1, 1, false, next::RESENT));
        push(&mut j.ctl, rec(1, 2, true, next::RETRY_AUTO));
        assert_eq!(pick(&j, Some(1)).map(|p| (p.generation, p.current)), Some((2, false)), "the generation that ran, not the re-sent one");
        assert_eq!(pick(&j, Some(2)).map(|p| (p.generation, p.current)), Some((3, true)));
        assert_eq!(pick(&j, None).map(|p| p.generation), Some(3));
        assert_eq!(pick(&j, Some(5)), None);
        assert_eq!(range_words(&j), "attempts 1-2");
        assert_eq!(range_words(&row(1, 1)), "attempt 1");
    }

    #[test]
    fn records_are_bounded_and_never_lost_by_a_stale_write() {
        let mut ctl = JobCtl::default();
        for g in 1..=(MAX_RECORDS as i64 + 5) {
            push(&mut ctl, rec(1 + g / 2, g, g % 2 == 0, next::RETRY_AUTO));
        }
        assert_eq!(ctl.attempts.len(), MAX_RECORDS);
        assert!(ctl.attempts.iter().all(|r| r.generation > 1), "the oldest never-started went first");
        assert_eq!(ctl.attempts.last().map(|r| r.generation), Some(MAX_RECORDS as i64 + 5));
        // Read before a requeue, written back after: the requeue's record stays.
        let mut stale = JobCtl::default();
        push(&mut stale, rec(1, 1, true, next::RETRY_AUTO));
        let mut stored = stale.clone();
        push(&mut stored, rec(2, 2, true, next::RETRY_AUTO));
        keep_newer(&stored, &mut stale);
        assert_eq!(stale.attempts.len(), 2);
        let mut newer = stored.clone();
        keep_newer(&stale, &mut newer);
        assert_eq!(newer, stored, "a newer list is kept as it is");
    }

    #[test]
    fn a_late_report_fills_only_an_empty_record() {
        let mut ctl = JobCtl::default();
        push(&mut ctl, rec(1, 1, true, next::RETRY_AUTO));
        let files = Outputs { artifacts: vec![ArtifactInfo { name: "verdict.txt".into(), sha256: "a".repeat(64), bytes: 3 }], ..Default::default() };
        assert!(!attach_outputs(&mut ctl, "n2", 1, files.clone()), "another node's report");
        assert!(!attach_outputs(&mut ctl, "n1", 2, files.clone()), "a generation with no record");
        assert!(attach_outputs(&mut ctl, "n1", 1, files.clone()));
        assert!(!attach_outputs(&mut ctl, "n1", 1, Outputs::default()), "the first report stands");
        assert_eq!(ctl.attempts[0].outputs.as_ref().map(|o| o.artifacts.len()), Some(1));
    }

    #[test]
    fn ends_and_next_steps_are_plain_words() {
        assert!(end_words("interrupted", Some("node_update"), true, "k2n7").contains("k2n7 stopped it while updating"));
        assert!(end_words("interrupted", Some("node_lost"), true, "k2n7").contains("offline"));
        assert_eq!(end_words("assigned", Some("withdraw_resend"), false, "k2n7"), "it never started on k2n7");
        assert!(end_words("failed", Some("weird_code"), true, "k2n7").contains("(weird_code)"));
        let d = end_words("failed", Some(crate::compute::proto::share::codes::DISK_BUDGET), true, "node-1");
        assert!(d.contains("whole disk budget") && !d.contains("infrastructure"), "the job's own result: {d}");
        let n = next_words(next::RETRY_AUTO, "k2n7", Some("k2n12"));
        assert!(n.starts_with("retried automatically on k2n12") && n.contains("--retries 0"), "{n}");
        assert!(next_words(next::RESENT, "k2n7", None).contains("new caps"));
        assert!(next_words(next::WITHDRAWN, "k2n7", Some("k2n9")).contains("to move it to k2n9"));
    }

    #[test]
    fn an_unanswered_withdraw_is_said_plainly_and_is_a_move() {
        let n = next_words(next::WITHDRAW_UNANSWERED, "node-a", Some("node-b"));
        assert_eq!(
            n,
            "node-a didn't answer a request to give it back within 3 min; K2 took it back (node-a is told not to run it) and placed it again on node-b (no retry used)"
        );
        assert_eq!(end_words("assigned", Some(next::WITHDRAW_UNANSWERED), false, "node-a"), "node-a didn't answer a request to give it back within 3 min");
        let m = move_words(next::WITHDRAW_UNANSWERED, "node-a", Some("node-b")).expect("a move");
        assert!(m.starts_with("node-a didn't answer") && !m.contains("infrastructure"), "{m}");
        assert_eq!(move_words(next::WITHDRAWN, "node-b-vin", Some("node-c-vin")).as_deref(), Some("node-b-vin gave it back from its queue before it started for node-c-vin"));
        for moved in [next::WITHDRAWN, next::NODE_LOST, next::NOT_ON_NODE, next::RESENT, next::PULLED_BACK] {
            assert!(move_words(moved, "a", None).is_some(), "{moved} is a move");
        }
        for retried in [next::RETRY_AUTO, next::RETRY_ASKED] {
            assert_eq!(move_words(retried, "a", None), None, "{retried} is a retry");
        }
    }

    #[test]
    fn a_record_keeps_the_outcome_not_the_pending_marker() {
        crate::tunnel::test_support::with_temp_home(a_record_keeps_the_outcome);
    }

    fn a_record_keeps_the_outcome() {
        let mut j = row(1, 1);
        j.state = "assigned".into();
        j.reason = Some(crate::compute::share::pending::WITHDRAW.into());
        let r = record_of(&j, next::WITHDRAW_UNANSWERED, 50);
        assert_eq!((r.reason.as_deref(), r.asked.as_deref(), r.next.as_str()), (Some("withdraw_unanswered"), Some("withdraw_requested"), "withdraw_unanswered"));
        j.reason = Some(crate::compute::share::pending::RESEND.into());
        let r = record_of(&j, next::RESENT, 50);
        assert_eq!((r.reason.as_deref(), r.asked.as_deref()), (Some("resent"), Some("withdraw_resend")));
        // A real end keeps its reason, with no ask.
        j.state = "interrupted".into();
        j.reason = Some("node_update".into());
        let r = record_of(&j, next::RETRY_AUTO, 50);
        assert_eq!((r.reason.as_deref(), r.asked), (Some("node_update"), None));
    }

    #[test]
    fn the_list_says_where_each_attempts_log_and_files_are() {
        crate::tunnel::test_support::with_temp_home(|| {
            let mut j = row(2, 2);
            let mut r1 = rec(1, 1, true, next::RETRY_AUTO);
            r1.outputs = Some(Outputs { artifacts: vec![ArtifactInfo { name: "v.txt".into(), sha256: "a".repeat(64), bytes: 3 }], ..Default::default() });
            push(&mut j.ctl, r1);
            j.node_id = "n2".into();
            assert!(logs::append(&j.id, 1, 1, crate::compute::proto::frames::LogStream::Out, b"verdict\n").unwrap());
            let names = HashMap::from([("n1".to_string(), "k2n7".to_string()), ("n2".to_string(), "k2n12".to_string())]);
            let list = list_json(&j, &names);
            assert_eq!(list.len(), 2);
            assert_eq!(list[0]["node"], "k2n7");
            assert_eq!(list[0]["log"]["available"], true);
            assert_eq!(list[0]["log"]["command"], "k2 compute logs jobid-00 --attempt 1");
            assert!(list[0]["next"].as_str().unwrap().starts_with("retried automatically on k2n12"));
            // No --artifact on this plan: nothing was asked.
            assert_eq!(list[0]["artifacts"]["status"], "not_asked");
            assert_eq!((list[1]["current"].as_bool(), list[1]["log"]["available"].as_bool()), (Some(true), Some(false)));
        });
    }
}
