//! `k2 compute stats <job>` (prd-compute-blacksmith-class-v1 §4.5.1, slice
//! O2). A reader of one job of the caller's own workspace (the owner and
//! Admins: any), behind the CI preview (`ciPreview`) like the rest of the
//! arc.
//!
//! The series comes from the node with the job's end (`Outputs.stats`,
//! feature `job-stats`, protocol 10) and is kept in `compute_job_stats`
//! (`k2_core::compute::job_stats`, 30 days). A job on an older k2-node has
//! none: the answer says so plainly (`stats: null` with `why`).

use std::collections::HashMap;

use k2_core::compute::{job_stats, store};
use serde_json::{json, Value};

use crate::compute_routes::{ci_gate, job_for, Who, R};
use k2_core::brand::CLI_NAME;
use k2_core::cli_name;

/// `GET /cli/compute/stats?job=<id>`.
pub const PATH: &str = "/cli/compute/stats";

/// `GET /cli/compute/stats?job=<id>`: the job's series, peaks, IO, held
/// time with what to ask for, OOM kills.
pub(crate) fn handle(who: &Who, params: &HashMap<String, String>) -> R<Value> {
    ci_gate(concat!(cli_name!(), " compute stats"))?;
    let j = job_for(who, params.get("job").map(String::as_str))?;
    let (stored, node) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (job_stats::get(&conn, &j.id), store::node_by_id(&conn, &j.node_id).map(|n| n.name))
    };
    let plan = j.plan();
    let cpu_millis = plan.as_ref().and_then(|p| p.limits.cpu_millis).filter(|m| *m > 0);
    let wall = match (j.started_at, j.ended_at) {
        (Some(s), Some(e)) if e >= s => Some(e - s),
        _ => None,
    };
    let ended = j.ended_at.is_some();
    let mut out = json!({
        "id": j.id,
        "short": k2_core::compute::message::short(&j.id),
        "state": j.state,
        "node": node,
        "generation": j.generation,
        "wallSecs": wall,
        "cpus": {
            "guaranteed": cpu_millis.map(|m| m as f64 / 1000.0),
            "burst": j.ctl.cpus_burst.map(|m| m as f64 / 1000.0),
        },
        "memLimitBytes": plan.as_ref().and_then(|p| p.limits.mem_bytes),
    });
    match stored {
        Some(s) => {
            // A series of an earlier attempt (a retry ran since) is still shown,
            // with its own wall time unknown here: its numbers stand alone.
            let wall = if s.generation == j.generation { wall } else { None };
            out["statsGeneration"] = json!(s.generation);
            out["stats"] = job_stats::view(&s.stats, cpu_millis, j.ctl.cpus_burst, wall);
        }
        None => {
            out["stats"] = Value::Null;
            out["why"] = json!(if !ended {
                concat!("it hasn't ended yet: the node sends the series with the job's end (", cli_name!(), " compute wait <job>, then ask again)").to_string()
            } else {
                format!(
                    "no series for it: its node's k2-node is older than protocol 10 ({CLI_NAME} compute node update {}), or it ended more than {} days ago",
                    node.as_deref().unwrap_or("<node>"),
                    job_stats::KEEP_SECS / 86_400
                )
            });
        }
    }
    // The lines `k2 compute stats` prints (the CLI only prints them).
    out["text"] = json!(job_stats::render(&out));
    Ok(out)
}
