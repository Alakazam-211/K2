//! The one result envelope agents read (research-distributed-compute-gaps
//! G1): `k2 compute run --json`, `k2 compute get --json` and the wake
//! message all describe a job with these facts. Stable field names;
//! additive changes only.
//!
//! Test names and failure messages inside `tests` are program output
//! (untrusted text). Everything else is ids, codes, numbers and hashes
//! the controller or the node's signed receipt produced.

use serde_json::{json, Value};

use super::proto::frames::SignedReceipt;
use super::proto::results::Outputs;
use super::store::JobRow;

/// The node-reported outputs stored on the job row, if any.
pub fn outputs(j: &JobRow) -> Option<Outputs> {
    j.summary_json.as_deref().and_then(|s| serde_json::from_str(s).ok())
}

/// A short stable id for the signed receipt: the first 16 hex of the
/// SHA-256 of its signed body.
pub fn receipt_id(j: &JobRow) -> Option<String> {
    let raw = j.receipt_json.as_deref()?;
    let r: SignedReceipt = serde_json::from_str(raw).ok()?;
    Some(super::proto::crypto::sha256_hex(r.body.as_bytes())[..16].to_string())
}

/// `ok`: the command ran, exited 0, and no collected test failed.
pub fn ok(j: &JobRow, o: Option<&Outputs>) -> bool {
    j.state == "done"
        && j.exit_code == Some(0)
        && o.and_then(|o| o.tests.as_ref()).is_none_or(|t| t.passed_all())
}

/// A split run's envelope: the parent's combined facts plus one line per
/// shard. `children` = (row, node name), any order.
pub fn envelope_with_shards(parent: &JobRow, children: &[(JobRow, Option<String>)]) -> Value {
    let mut v = envelope(parent, None);
    let mut rows: Vec<&(JobRow, Option<String>)> = children.iter().collect();
    rows.sort_by_key(|(j, _)| j.ctl.shard.as_ref().map(|s| s.index).unwrap_or(0));
    let shards: Vec<Value> = rows
        .iter()
        .map(|(j, node)| {
            let t = outputs(j).and_then(|o| o.tests);
            json!({
                "index": j.ctl.shard.as_ref().map(|s| s.index),
                "job": j.id,
                "short": super::message::short(&j.id),
                "node": node,
                "state": j.state,
                "exit": j.exit_code,
                "reason": j.reason,
                "attempt": j.attempt,
                "durationSecs": match (j.started_at, j.ended_at) { (Some(a), Some(b)) => Some(b - a), _ => None },
                "tests": t.map(|t| json!({"passed": t.passed, "failed": t.failed, "errors": t.errors, "skipped": t.skipped})),
                "artifacts": outputs(j).map(|o| o.artifacts.len()).unwrap_or(0),
            })
        })
        .collect();
    v["node"] = Value::Null;
    v["shards"] = shards.into();
    v
}

/// The envelope for one job attempt. `node` is the node's name.
pub fn envelope(j: &JobRow, node: Option<&str>) -> Value {
    let o = outputs(j);
    let short = super::message::short(&j.id).to_string();
    let duration = match (j.started_at, j.ended_at) {
        (Some(a), Some(b)) => Some(b - a),
        _ => None,
    };
    let queue = j.started_at.or(j.assigned_at).map(|s| (s - j.created_at).max(0));
    let artifacts: Vec<Value> = o
        .as_ref()
        .map(|o| {
            o.artifacts
                .iter()
                .map(|a| json!({"name": a.name, "sha256": a.sha256, "bytes": a.bytes}))
                .collect()
        })
        .unwrap_or_default();
    let mut next: Vec<String> = Vec::new();
    if j.is_terminal() {
        if o.as_ref().and_then(|o| o.tests.as_ref()).is_some_and(|t| !t.passed_all()) || (j.state == "done" && j.exit_code != Some(0)) {
            next.push(format!("k2 compute logs {short} --failures"));
        }
        if !artifacts.is_empty() {
            next.push(format!("k2 compute fetch {short} --out <dir>"));
        }
        if j.state != "done" {
            next.push(format!("k2 compute retry {short}"));
        }
    } else {
        next.push(format!("k2 compute attach {short}"));
    }
    json!({
        "job": j.id,
        "short": short,
        "node": node,
        "state": j.state,
        "ok": ok(j, o.as_ref()),
        "exit": j.exit_code,
        "signal": j.signal,
        "reason": j.reason,
        "detail": j.detail,
        "attempt": j.attempt,
        "generation": j.generation,
        "argv": j.argv,
        "commit": j.src_commit,
        "tree": j.src_tree,
        "createdAt": j.created_at,
        "startedAt": j.started_at,
        "endedAt": j.ended_at,
        "durationSecs": duration,
        "queueSecs": queue,
        "artifacts": artifacts,
        "tests": o.as_ref().and_then(|o| o.tests.clone()),
        "notes": o.as_ref().map(|o| o.notes.clone()).unwrap_or_default(),
        "receipt": j.receipt_json.as_ref().map(|_| json!({"id": receipt_id(j), "verified": j.receipt_ok == Some(true)})),
        "next": next,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::proto::results::{ArtifactInfo, FailingTest, TestSummary};

    fn row() -> JobRow {
        JobRow {
            id: "abcdef0123456789".into(),
            client_job_id: "c".into(),
            node_id: "n".into(),
            workspace_id: "w".into(),
            session_id: None,
            requested_by: "agent:w".into(),
            argv: vec!["cargo".into(), "nextest".into()],
            env_names: vec![],
            request_sha256: "h".into(),
            plan_json: "{}".into(),
            src_commit: Some("c".repeat(40)),
            src_tree: None,
            dirty_sha256: None,
            state: "done".into(),
            reason: None,
            detail: None,
            attempt: 1,
            generation: 1,
            plan_digest: "d".into(),
            exclusive: false,
            detach: false,
            retry_interrupted: false,
            exit_code: Some(100),
            signal: None,
            created_at: 10,
            assigned_at: Some(12),
            started_at: Some(15),
            ended_at: Some(75),
            log_seq: 3,
            log_bytes: 9,
            summary_json: None,
            receipt_json: None,
            receipt_ok: None,
            notified_at: None,
            ctl: Default::default(),
            ..Default::default()
        }
    }

    #[test]
    fn envelope_carries_exit_duration_artifacts_tests_and_receipt() {
        let mut j = row();
        let o = Outputs {
            artifacts: vec![ArtifactInfo { name: "target/nextest/ci/junit.xml".into(), sha256: "s".into(), bytes: 42 }],
            tests: Some(TestSummary {
                total: 3,
                passed: 2,
                failed: 1,
                failing: vec![FailingTest { name: "t".into(), classname: None, file: None, kind: "failure".into(), message: "m".into(), shard: None }],
                ..Default::default()
            }),
            notes: vec![],
            ..Default::default()
        };
        j.summary_json = Some(serde_json::to_string(&o).unwrap());
        let signed = SignedReceipt { body: "{\"v\":1}".into(), sig: "x".into() };
        j.receipt_json = Some(serde_json::to_string(&signed).unwrap());
        j.receipt_ok = Some(true);
        let v = envelope(&j, Some("mini-1"));
        assert_eq!(v["exit"], 100);
        assert_eq!(v["ok"], false);
        assert_eq!(v["durationSecs"], 60);
        assert_eq!(v["queueSecs"], 5);
        assert_eq!(v["node"], "mini-1");
        assert_eq!(v["artifacts"][0]["bytes"], 42);
        assert_eq!(v["tests"]["failed"], 1);
        assert_eq!(v["receipt"]["verified"], true);
        assert_eq!(v["receipt"]["id"].as_str().unwrap().len(), 16);
        assert_eq!(v["next"][0], "k2 compute logs abcdef01 --failures");
        assert_eq!(v["next"][1], "k2 compute fetch abcdef01 --out <dir>");
    }

    #[test]
    fn ok_needs_exit_zero_and_no_failed_test() {
        let mut j = row();
        j.exit_code = Some(0);
        assert!(ok(&j, None));
        let bad = Outputs { tests: Some(TestSummary { failed: 1, ..Default::default() }), ..Default::default() };
        assert!(!ok(&j, Some(&bad)), "a JUnit failure under exit 0 is not ok");
        j.state = "failed".into();
        assert!(!ok(&j, None));
        let v = envelope(&j, None);
        assert_eq!(v["next"].as_array().unwrap().last().unwrap(), "k2 compute retry abcdef01");
    }
}
