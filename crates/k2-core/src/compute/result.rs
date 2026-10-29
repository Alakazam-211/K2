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

/// The state every view shows (CE-A7): a job held in a shared node's queue
/// is `assigned` in the row but `queued` everywhere it is shown, the
/// top-level `state` of `get --json` and `result.state` alike.
pub fn shown_state(j: &JobRow) -> &str {
    if j.node_queued && j.state == "assigned" {
        "queued"
    } else {
        j.state.as_str()
    }
}

/// The outputs as raw JSON: fields a newer node adds (`tools_used`,
/// `resources`, `host`) are read from here, so an older controller build
/// never has to know their exact Rust type.
fn outputs_raw(j: &JobRow) -> Option<Value> {
    j.summary_json.as_deref().and_then(|s| serde_json::from_str(s).ok())
}

/// `snake_case` object keys to `camelCase`, all the way down (the node's
/// outputs are snake_case; the result envelope is camelCase).
fn camel(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| {
                    let mut out = String::new();
                    let mut up = false;
                    for c in k.chars() {
                        if c == '_' {
                            up = true;
                        } else if up {
                            out.extend(c.to_uppercase());
                            up = false;
                        } else {
                            out.push(c);
                        }
                    }
                    (out, camel(v))
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(camel).collect()),
        other => other.clone(),
    }
}

/// `result.tools` (CE-4): the exact tool versions. A node with the
/// `tools-used` feature reports what the job's own PATH resolved
/// (`basis: "job"`); for an older node, the node-wide probe from the
/// signed receipt (`basis: "node-probe"`: what the node had, not
/// necessarily what the job used). Null when neither is known.
pub fn tools(j: &JobRow) -> Value {
    if let Some(m) = outputs_raw(j).and_then(|o| o.get("tools_used").cloned()).filter(|m| m.as_object().is_some_and(|m| !m.is_empty())) {
        return json!({"basis": "job", "versions": m});
    }
    let probe = j
        .receipt_json
        .as_deref()
        .and_then(|r| serde_json::from_str::<SignedReceipt>(r).ok())
        .and_then(|s| serde_json::from_str::<Value>(&s.body).ok())
        .and_then(|b| b.get("tools").cloned())
        .filter(|m| m.as_object().is_some_and(|m| !m.is_empty()));
    match probe {
        Some(m) => json!({"basis": "node-probe", "versions": m}),
        None => Value::Null,
    }
}

/// `result.resources` (CE-14): `{peakMemBytes, cpuSecs, memSource}` from a
/// node with `job-resources`; null from an older node.
pub fn resources(j: &JobRow) -> Value {
    outputs_raw(j).and_then(|o| o.get("resources").filter(|r| r.is_object()).map(camel)).unwrap_or(Value::Null)
}

/// `result.host` (CE-19): the node's load stamp at job start and end
/// (`{start: {load1, cpus, steal, jobs}, end: …}`); null from an older node.
pub fn host(j: &JobRow) -> Value {
    outputs_raw(j).and_then(|o| o.get("host").filter(|h| h.is_object()).map(camel)).unwrap_or(Value::Null)
}

/// The peak memory a job used, when its node reported it (CE-14).
pub fn peak_mem_bytes(j: &JobRow) -> Option<u64> {
    resources(j).get("peakMemBytes").and_then(Value::as_u64)
}

/// Who sent a job, for listings (CE-9): `agent:<ws>`, `shell:<ws>`,
/// `owner` or `login:<name>` (the job's `requested_by`).
pub fn submitted_by(j: &JobRow) -> &str {
    j.requested_by.as_str()
}

/// `Some("owner_token")` when the job came in on the owner token (CE-9):
/// an agent whose passport vars were missing falls back to it silently,
/// so the mark makes that visible.
pub fn via(j: &JobRow) -> Option<&'static str> {
    (j.requested_by == "owner").then_some("owner_token")
}

/// CE-5: which time to keep for a node observation that carries the node's
/// own time `at`, heard by this server at `now`. `skew` = the node's clock
/// minus this server's, when known (a shared node's share view, or a live
/// sample). Returns (the time to store, whose clock, seconds heard late).
///
/// - Skew known and within [`super::store::REPORTED_LATE_SECS`]: the node's
///   time (never in the future); heard more than that late → late secs.
/// - Skew known and larger: this server's time (`clock: "server"`).
/// - Skew unknown: the node's time only when it agrees with `now` within
///   the limit (then it isn't late either); else this server's time.
pub fn node_time(at: i64, now: i64, skew: Option<i64>) -> (i64, &'static str, Option<i64>) {
    let lim = super::store::REPORTED_LATE_SECS;
    match skew {
        Some(s) if s.abs() <= lim => {
            let t = at.min(now);
            let late = now - t;
            (t, "node", (late > lim).then_some(late))
        }
        Some(_) => (now, "server", None),
        None if (now - at).abs() <= lim => (at.min(now), "node", None),
        None => (now, "server", None),
    }
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
        "state": shown_state(j),
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
        // prd-compute-easy-mode-0467: additive, always present (null when
        // unknown or from an older node).
        "tag": j.ctl.tag,
        "submittedBy": submitted_by(j),
        "via": via(j),
        "clock": j.ctl.clock,
        "reportedLateSecs": j.ctl.reported_late_secs,
        "heardAt": j.ctl.heard_at,
        "effective": j.ctl.effective,
        "tools": tools(j),
        "resources": resources(j),
        "host": host(j),
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
    fn result_state_matches_the_shown_state() {
        // CE-A7: a node-queued row is `assigned` in the table; `get --json`
        // showed `queued` at the top and `assigned` in `result.state`.
        let mut j = row();
        j.state = "assigned".into();
        j.node_queued = true;
        assert_eq!(shown_state(&j), "queued");
        assert_eq!(envelope(&j, None)["state"], "queued");
        j.node_queued = false;
        assert_eq!(envelope(&j, None)["state"], "assigned");
    }

    #[test]
    fn node_time_rules() {
        // Heard 3 h late from a node whose clock agrees: the node's time,
        // reported late by 10800 s.
        assert_eq!(node_time(1_000, 11_800, Some(3)), (1_000, "node", Some(10_800)));
        // Heard on time: no late note.
        assert_eq!(node_time(1_000, 1_030, Some(0)), (1_000, "node", None));
        // A skewed node: this server's time.
        assert_eq!(node_time(1_000, 11_800, Some(600)), (11_800, "server", None));
        // Skew unknown: the node's time only when it agrees with now.
        assert_eq!(node_time(1_000, 1_050, None), (1_000, "node", None));
        assert_eq!(node_time(1_000, 11_800, None), (11_800, "server", None));
        // Never in the future.
        assert_eq!(node_time(1_100, 1_000, Some(10)), (1_000, "node", None));
    }

    #[test]
    fn new_fields_from_a_newer_node_and_fallbacks() {
        let mut j = row();
        // Nothing reported: null, never a made-up value.
        let v = envelope(&j, None);
        for k in ["tools", "resources", "host", "tag", "effective", "reportedLateSecs", "clock", "via"] {
            assert!(v.get(k).is_some_and(Value::is_null), "{k} present and null: {v}");
        }
        assert_eq!(v["submittedBy"], "agent:w");
        // An older node: the receipt's node-wide probe.
        let body = json!({"tools": {"rustc": "1.99.1"}}).to_string();
        j.receipt_json = Some(serde_json::to_string(&SignedReceipt { body, sig: "x".into() }).unwrap());
        assert_eq!(tools(&j), json!({"basis": "node-probe", "versions": {"rustc": "1.99.1"}}));
        // A `tools-used` / `job-resources` node: what the job used.
        j.summary_json = Some(
            json!({"artifacts": [], "notes": [], "tools_used": {"rustc": "1.99.0"},
                   "resources": {"peak_mem_bytes": 1u64 << 30, "cpu_secs": 12.5, "mem_source": "memory.peak"},
                   "host": {"start": {"load1": 0.5, "cpus": 12, "steal": null}}})
            .to_string(),
        );
        assert_eq!(tools(&j), json!({"basis": "job", "versions": {"rustc": "1.99.0"}}));
        assert_eq!(resources(&j), json!({"peakMemBytes": 1u64 << 30, "cpuSecs": 12.5, "memSource": "memory.peak"}));
        assert_eq!(peak_mem_bytes(&j), Some(1 << 30));
        assert_eq!(host(&j)["start"]["load1"], 0.5);
        j.requested_by = "owner".into();
        j.ctl.tag = Some("fuzz:p3".into());
        let v = envelope(&j, None);
        assert_eq!((v["via"].as_str(), v["tag"].as_str()), (Some("owner_token"), Some("fuzz:p3")));
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
