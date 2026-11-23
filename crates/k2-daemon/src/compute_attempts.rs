//! A job's earlier attempts, readable after K2 retried or moved it
//! (`k2_core::compute::attempts`): the list `get`, `get --json` and `logs`
//! carry, `GET /cli/compute/jobs/logs` with `attempt=N`, and the attempt a
//! `fetch --attempt N` reads (its node and generation).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use k2_core::compute::attempts::{self, Pick};
use k2_core::compute::proto::crypto;
use k2_core::compute::{self as compute, codes, logs, store};

use crate::compute_routes::{job_for, no, Who, R};
use crate::compute_views::{job_summary, queue_view};
use crate::compute_ws;

/// The job's attempts, oldest first (the current one last).
pub(crate) fn list(j: &store::JobRow) -> serde_json::Value {
    let names: HashMap<String, String> = k2_core::db::read(|conn| store::list_nodes(conn, true)).into_iter().map(|n| (n.id, n.name)).collect();
    attempts::list_json(j, &names).into()
}

/// `attempt=N` of a request: the attempt to read (the current one when
/// absent). 400 for a malformed number, 404 for one the job never had.
pub(crate) fn picked(j: &store::JobRow, params: &HashMap<String, String>) -> R<Pick> {
    let n = match params.get("attempt").map(|a| a.trim()).filter(|a| !a.is_empty()) {
        None => None,
        Some(a) => Some(a.parse::<i64>().ok().filter(|n| *n >= 1).ok_or_else(|| no(codes::BAD_REQUEST, 400, "attempt is a number from 1"))?),
    };
    attempts::pick(j, n).ok_or_else(|| {
        let short = compute::message::short(&j.id);
        no("attempt_not_found", 404, format!("job {short} has {} (k2 compute get {short} lists them)", attempts::range_words(j)))
    })
}

/// The job as attempt `p` saw it: its node, generation, state and files,
/// for the artifact routes (a pull goes to that node, for that generation).
pub(crate) fn as_attempt(j: &store::JobRow, p: &Pick) -> store::JobRow {
    if p.current {
        return j.clone();
    }
    let mut v = j.clone();
    v.node_id = p.node_id.clone();
    v.generation = p.generation;
    v.attempt = p.attempt;
    v.state = p.state.clone();
    v.summary_json = p.outputs.as_ref().and_then(|o| serde_json::to_string(o).ok());
    v
}

/// `GET /cli/compute/jobs/logs?job=…[&attempt=N][&generation=G][&cursor=…|&since_seq=…][&wait=…]`:
/// one attempt's log, the latest by default. Every answer names the
/// attempt and generation it reads and lists the job's attempts.
pub(crate) fn job_logs(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    let j = job_for(who, params.get("job").map(String::as_str))?;
    compute_ws::touch_follower(&j.id);
    if !j.ctl.shards.is_empty() {
        let shards: Vec<String> = j.ctl.shards.clone();
        return Ok(serde_json::json!({
            "job": job_summary(&j),
            "generation": j.generation,
            "cursor": 0,
            "chunks": [],
            "end": j.is_terminal(),
            "shards": shards,
            "hint": "a split run: each shard has its own log (k2 compute logs <run> reads them all)",
        }));
    }
    // `generation=` (a follower that keeps to one attempt) wins over `attempt=`.
    let (attempt, generation) = match params.get("generation").and_then(|g| g.parse::<i64>().ok()) {
        Some(g) => (j.ctl.attempts.iter().find(|r| r.generation == g).map(|r| r.attempt).unwrap_or(j.attempt), g),
        None => {
            let p = picked(&j, params)?;
            (p.attempt, p.generation)
        }
    };
    let mut cursor = match params.get("cursor").and_then(|c| c.parse::<u64>().ok()) {
        Some(c) => c,
        None => logs::cursor_after_seq(&j.id, generation, params.get("since_seq").and_then(|s| s.parse().ok()).unwrap_or(0)),
    };
    let max = params.get("max").and_then(|m| m.parse::<usize>().ok()).unwrap_or(256 * 1024).clamp(1024, 1 << 20);
    let wait = params.get("wait").and_then(|w| w.parse::<u64>().ok()).unwrap_or(0).min(25);
    let deadline = Instant::now() + Duration::from_secs(wait);
    // Woken by this job's own chunks and state only (never another job's),
    // and each re-read is a pooled read, never the main lock (0.46.17).
    let watch = compute_ws::JobWatch::new();
    watch.watch(vec![j.id.clone()], Vec::new());
    loop {
        let seen = watch.seen();
        let (recs, next) = logs::read_from(&j.id, generation, cursor, max);
        let row = k2_core::db::read(|conn| store::job_by_id(conn, &j.id)).ok_or_else(|| no(codes::JOB_NOT_FOUND, 404, "the job vanished"))?;
        let terminal = row.is_terminal() || row.generation != generation;
        let now = Instant::now();
        if !recs.is_empty() || terminal || now >= deadline {
            cursor = next;
            let chunks: Vec<serde_json::Value> = recs
                .iter()
                .map(|r| serde_json::json!({"seq": r.seq, "stream": logs::stream_name(r.stream), "data": crypto::b64(&r.data)}))
                .collect();
            let more = !logs::read_from(&j.id, generation, cursor, 1).0.is_empty();
            let mut out = serde_json::json!({
                "job": job_summary(&row),
                "attempt": attempt,
                "generation": generation,
                "cursor": cursor,
                "chunks": chunks,
                "end": terminal && !more,
                "nodeOnline": compute_ws::is_online(&row.node_id),
                "attempts": list(&row),
            });
            // The wait line a follower prints (L6): why it waits and where,
            // on this server or in the node's own queue.
            if row.state == "queued" {
                if let Some(q) = queue_view(who, &row, &compute_ws::decision_for(&row.node_id)) {
                    out["queue"] = q;
                }
            } else if row.node_queued {
                out["queue"] = out["job"]["queue"].clone();
            }
            if let Some(q) = out.get_mut("queue") {
                crate::compute_views::with_elsewhere(&row, q);
            }
            return Ok(out);
        }
        watch.wait(seen, deadline - now);
    }
}
