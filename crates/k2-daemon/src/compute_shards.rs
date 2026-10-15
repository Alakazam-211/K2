//! Split runs ("compute B", research-distributed-compute-gaps G8): the
//! parent row's life. Each shard is an ordinary `--any` job; when the
//! last one ends, the parent takes the combined result
//! ([`k2_core::compute::shards::aggregate`]) and the asking session gets
//! ONE wake message for the whole run.

use k2_core::compute::shards;
use k2_core::compute::store::{self, JobRow};

use crate::compute_ws;

/// The parent's shards with their node names, shard 1 first.
pub fn children(parent: &JobRow) -> Vec<(JobRow, Option<String>)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    parent
        .ctl
        .shards
        .iter()
        .filter_map(|id| store::job_by_id(&conn, id))
        .map(|j| {
            let name = store::node_by_id(&conn, &j.node_id).map(|n| n.name);
            (j, name)
        })
        .collect()
}

/// A shard reached a terminal state: if it was the last one, end the
/// parent and send its one message.
pub fn child_ended(parent_id: &str) {
    let Some(parent) = ({
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::job_by_id(&conn, parent_id)
    }) else {
        return;
    };
    if parent.is_terminal() || parent.ctl.shards.is_empty() {
        return;
    }
    let kids = children(&parent);
    if kids.len() != parent.ctl.shards.len() {
        return;
    }
    let indexed: Vec<(u32, &JobRow)> = kids.iter().map(|(j, _)| (j.ctl.shard.as_ref().map(|s| s.index).unwrap_or(0), j)).collect();
    let Some(agg) = shards::aggregate(&indexed) else { return };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Err(e) = store::finish_parent(&conn, &parent.id, &agg) {
            k2_core::log_debug!("[compute] finishing split run {}: {e}", parent.id);
            return;
        }
        store::record_event(
            &conn,
            "controller",
            "job_end",
            None,
            Some(&parent.workspace_id),
            Some(&parent.id),
            serde_json::json!({"state": agg.state, "reason": agg.reason, "exit": agg.exit_code, "shards": kids.len()}),
        );
    }
    compute_ws::finalize(&parent.id);
    compute_ws::notify_change();
}
