//! Suite splitting = "compute B" (research-distributed-compute-gaps G8):
//! one parent job, N child jobs (`--any`, one per shard, spread across
//! nodes), one combined result and one wake message.
//!
//! The parent is a `compute_jobs` row with no node (`node_id = ""`) in
//! state `running` until every child has ended; its options list the
//! children (`JobCtl::shards`), each child names its parent and index
//! (`JobCtl::shard`). Pure here: placeholders and the aggregate.

use super::proto::results::{self, TestSummary};
use super::store::JobRow;

/// Replaced by the 1-based shard index in each child's argv.
pub const SHARD: &str = "{shard}";
/// Replaced by the shard count.
pub const SHARDS: &str = "{shards}";
pub const MAX_SHARDS: u32 = 16;

/// The argv names both placeholders (else every child would run the same
/// thing).
pub fn has_placeholders(argv: &[String]) -> bool {
    argv.iter().any(|a| a.contains(SHARD)) && argv.iter().any(|a| a.contains(SHARDS))
}

/// Child `index` (1-based) of `of`.
pub fn argv_for(argv: &[String], index: u32, of: u32) -> Vec<String> {
    // `{shards}` first: `{shard}` is a prefix of it.
    argv.iter().map(|a| a.replace(SHARDS, &of.to_string()).replace(SHARD, &index.to_string())).collect()
}

/// The parent's combined result once every child has ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aggregate {
    pub state: String,
    pub exit_code: Option<i64>,
    pub signal: Option<i64>,
    pub reason: Option<String>,
    pub detail: Option<String>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub tests: Option<TestSummary>,
}

/// `children` = (1-based index, row), any order. `None` while one runs.
///
/// - every shard `done`: `done`, exit = the first non-zero exit by index
///   (else 0); a shard killed by a signal makes the whole `done` with
///   that signal and no exit;
/// - otherwise the first shard (by index) that didn't finish its command
///   decides: its state and reason, detail `shard i/N: …`;
/// - tests: every shard's JUnit merged, failures tagged with their shard.
pub fn aggregate(children: &[(u32, &JobRow)]) -> Option<Aggregate> {
    if children.is_empty() || children.iter().any(|(_, j)| !j.is_terminal()) {
        return None;
    }
    let mut sorted: Vec<&(u32, &JobRow)> = children.iter().collect();
    sorted.sort_by_key(|(i, _)| *i);
    let of = sorted.len();
    let started_at = sorted.iter().filter_map(|(_, j)| j.started_at).min();
    let ended_at = sorted.iter().filter_map(|(_, j)| j.ended_at).max();
    let parts: Vec<(u32, TestSummary)> = sorted
        .iter()
        .filter_map(|(i, j)| super::result::outputs(j).and_then(|o| o.tests).map(|t| (*i, t)))
        .collect();
    let tests = (!parts.is_empty()).then(|| results::merge_shards(&parts.iter().map(|(i, t)| (*i, t)).collect::<Vec<_>>()));
    let base = Aggregate { state: "done".into(), exit_code: Some(0), signal: None, reason: None, detail: None, started_at, ended_at, tests };
    if let Some((i, j)) = sorted.iter().find(|(_, j)| j.state != "done") {
        let what = j.detail.clone().or_else(|| j.reason.clone()).unwrap_or_else(|| j.state.clone());
        return Some(Aggregate {
            state: j.state.clone(),
            exit_code: None,
            reason: j.reason.clone(),
            detail: Some(format!("shard {i}/{of}: {what}")),
            ..base
        });
    }
    if let Some((_, j)) = sorted.iter().find(|(_, j)| j.exit_code.is_none()) {
        return Some(Aggregate { exit_code: None, signal: j.signal, ..base });
    }
    let exit = sorted.iter().map(|(_, j)| j.exit_code.unwrap_or(0)).find(|c| *c != 0).unwrap_or(0);
    Some(Aggregate { exit_code: Some(exit), ..base })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::proto::results::{FailingTest, Outputs};

    fn child(state: &str, exit: Option<i64>, tests: Option<(u64, u64)>) -> JobRow {
        let mut j = blank();
        j.state = state.into();
        j.exit_code = exit;
        j.started_at = Some(10);
        j.ended_at = Some(20);
        if let Some((p, f)) = tests {
            let o = Outputs {
                tests: Some(TestSummary {
                    total: p + f,
                    passed: p,
                    failed: f,
                    failing: (0..f).map(|k| FailingTest { name: format!("t{k}"), classname: None, file: None, kind: "failure".into(), message: "m".into(), shard: None }).collect(),
                    ..Default::default()
                }),
                ..Default::default()
            };
            j.summary_json = Some(serde_json::to_string(&o).unwrap());
        }
        j
    }

    fn blank() -> JobRow {
        JobRow {
            id: "c".into(),
            client_job_id: "c".into(),
            node_id: "n".into(),
            workspace_id: "w".into(),
            session_id: None,
            requested_by: "o".into(),
            argv: vec![],
            env_names: vec![],
            request_sha256: "h".into(),
            plan_json: "{}".into(),
            src_commit: None,
            src_tree: None,
            dirty_sha256: None,
            state: "queued".into(),
            reason: None,
            detail: None,
            attempt: 1,
            generation: 1,
            plan_digest: "d".into(),
            exclusive: false,
            detach: true,
            retry_interrupted: false,
            exit_code: None,
            signal: None,
            created_at: 1,
            assigned_at: None,
            started_at: None,
            ended_at: None,
            log_seq: 0,
            log_bytes: 0,
            summary_json: None,
            receipt_json: None,
            receipt_ok: None,
            notified_at: None,
            ctl: Default::default(),
        }
    }

    #[test]
    fn placeholders_substitute_per_shard() {
        let argv: Vec<String> = ["cargo", "nextest", "run", "--partition", "hash:{shard}/{shards}"].iter().map(|s| s.to_string()).collect();
        assert!(has_placeholders(&argv));
        assert_eq!(argv_for(&argv, 3, 4)[4], "hash:3/4");
        assert!(!has_placeholders(&argv[..4]));
        assert!(!has_placeholders(&["x{shard}".to_string()]), "both are needed");
    }

    #[test]
    fn all_done_passes_or_takes_the_first_failing_exit() {
        let a = child("done", Some(0), Some((10, 0)));
        let b = child("done", Some(100), Some((5, 2)));
        let c = child("done", Some(101), None);
        let agg = aggregate(&[(3, &c), (1, &a), (2, &b)]).unwrap();
        assert_eq!((agg.state.as_str(), agg.exit_code), ("done", Some(100)), "first non-zero by shard index");
        let t = agg.tests.unwrap();
        assert_eq!((t.passed, t.failed), (15, 2));
        assert!(t.failing.iter().all(|f| f.shard == Some(2)));
        let ok = aggregate(&[(1, &a), (2, &child("done", Some(0), None))]).unwrap();
        assert_eq!(ok.exit_code, Some(0));
        assert_eq!((ok.started_at, ok.ended_at), (Some(10), Some(20)));
    }

    #[test]
    fn a_running_shard_holds_and_an_infra_end_decides() {
        let a = child("done", Some(0), None);
        assert!(aggregate(&[(1, &a), (2, &child("running", None, None))]).is_none());
        let mut lost = child("interrupted", None, None);
        lost.reason = Some("node_lost".into());
        let agg = aggregate(&[(1, &a), (2, &lost), (3, &child("failed", None, None))]).unwrap();
        assert_eq!((agg.state.as_str(), agg.reason.as_deref(), agg.detail.as_deref()), ("interrupted", Some("node_lost"), Some("shard 2/3: node_lost")));
        assert!(aggregate(&[]).is_none());
    }
}
