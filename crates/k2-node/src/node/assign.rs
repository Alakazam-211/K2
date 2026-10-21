//! `assign` and `cancel` from a server, and the state frames that answer
//! them. Everything acts on the session's tenant: its pin (the plan must
//! name this node as that server knows it), its ledger, and its slot in
//! the running-jobs map (V48).
//!
//! Solo mode: an assign starts or is refused, as today. Lane S2 adds the
//! node queue here (accept into the queue, `withdraw` / `if_queued`).

use std::sync::Arc;

use k2_node_proto::frames::{Assign, Frame, JobState, StateUpdate};
use k2_node_proto::refusal::{self, JobAsk, NodeView};
use tokio::sync::watch;

use super::{Node, RunningJob, StopReason, Tenant};

impl Node {
    pub(super) fn state_frame(&self, job_id: &str, generation: u32, state: JobState, reason: Option<&str>, detail: Option<&str>) -> Frame {
        Frame::State(StateUpdate {
            queue: None,
            job_id: job_id.to_string(),
            generation,
            state,
            reason: reason.map(str::to_string),
            detail: detail.map(str::to_string),
            at: self.now(),
            exit: None,
            src: None,
            log_seq: 0,
            outputs: None,
        })
    }

    pub fn row_state_frame(&self, r: &crate::ledger::Row) -> Frame {
        Frame::State(StateUpdate {
            queue: None,
            job_id: r.job_id.clone(),
            generation: r.generation,
            state: r.state,
            reason: r.reason.clone(),
            detail: r.detail.clone(),
            at: r.ended_at.or(r.started_at).unwrap_or(r.created_at),
            exit: r.exit.clone(),
            src: r.src.clone(),
            log_seq: r.log_seq,
            outputs: r.outputs.clone(),
        })
    }

    /// Handle an `assign` from `t`'s server. Returns `Some((job_id,
    /// generation))` when a new attempt started (the session tracks its
    /// logs).
    pub fn on_assign(self: &Arc<Self>, t: &Arc<Tenant>, a: Assign) -> Option<(String, u32)> {
        let job = a.fence.job_id.clone();
        let gen = a.fence.generation;
        if !a.consistent() {
            t.hub.send(self.state_frame(&job, gen, JobState::Failed, Some("plan_mismatch"), Some("plan digest or job id doesn't match the fence")));
            return None;
        }
        if a.plan.node_id != t.node_id() {
            t.hub.send(self.state_frame(&job, gen, JobState::Failed, Some("wrong_node"), Some("this plan names another node")));
            return None;
        }
        // No blind replay: a known fence (or a newer generation) only
        // reports its state.
        let known = {
            let l = t.ledger.lock().unwrap();
            match l.get(&job, gen) {
                Ok(Some(r)) => Some(r),
                _ => l.latest(&job).ok().flatten().filter(|r| r.generation > gen),
            }
        };
        if let Some(r) = known {
            t.hub.send(self.row_state_frame(&r));
            return None;
        }
        if let Some(r) = self.assign_refusal(t, &a) {
            t.hub.send(self.state_frame(&job, gen, JobState::Queued, Some(&r.code), Some(&r.message)));
            return None;
        }
        if let Err(e) = t.ledger.lock().unwrap().insert(&a, self.opts.clock.as_ref()) {
            t.hub.send(self.state_frame(&job, gen, JobState::Failed, Some("ledger_error"), Some(&e)));
            return None;
        }
        let (stop_tx, stop_rx) = watch::channel(None);
        self.jobs.lock().unwrap().insert(
            (t.slot, job.clone()),
            RunningJob {
                generation: gen,
                cpu_millis: a.plan.limits.cpu_millis,
                mem_bytes: a.plan.limits.mem_bytes,
                exclusive: a.plan.exclusive,
                stop: stop_tx,
            },
        );
        self.jobs_changed();
        let (node, tenant) = (self.clone(), t.clone());
        tokio::spawn(async move { node.run_job(tenant, a, stop_rx).await });
        Some((job, gen))
    }

    fn assign_refusal(&self, t: &Tenant, a: &Assign) -> Option<k2_node_proto::frames::Refusal> {
        let l = self.local.lock().unwrap().clone();
        let (cap_cpu, cap_mem, par) = self.caps(t, &l);
        let (used_cpu, used_mem, running, excl) = self.used(cap_cpu, cap_mem, par);
        let (disk_free, _) = crate::sysinfo::disk(&self.layout().home);
        let tools = self.tools.lock().unwrap().clone();
        let view = NodeView {
            control: self.effective_control(&l),
            unavailable: &l.unavailable,
            foreign_lock: l.locks.holding.as_deref(),
            disk_free_bytes: disk_free,
            disk_floor_bytes: l.policy.disk_floor_bytes,
            workspaces_allow: &l.policy.workspaces,
            tools: &tools,
            running,
            parallel_max: par,
            exclusive_running: excl,
            free_cpu_millis: cap_cpu.saturating_sub(used_cpu),
            free_mem_bytes: cap_mem.saturating_sub(used_mem),
            network_none: l.policy.network == "none",
            network_none_enforced: self.wall.network_none(),
        };
        let ask = JobAsk {
            workspace_id: &a.plan.workspace_id,
            cpu_millis: a.plan.limits.cpu_millis,
            mem_bytes: a.plan.limits.mem_bytes,
            // `limits.disk_bytes` is the grant's ceiling for this job, not
            // space it needs free right now; a start only needs the node's
            // floor (checked in `node_refusal`). Requiring the ceiling free
            // refused every job on a node with less than 60 GB free.
            disk_bytes: 0,
            exclusive: a.plan.exclusive,
            needs_git: a.plan.src.is_some(),
        };
        refusal::refusal(&view, &ask)
    }

    /// `cancel` from `t`'s server: only ever its own jobs.
    pub fn cancel(&self, t: &Tenant, job_id: &str, generation: u32, reason: &str) {
        let jobs = self.jobs.lock().unwrap();
        if let Some(j) = jobs.get(&(t.slot, job_id.to_string())).filter(|j| j.generation == generation) {
            j.stop.send_replace(Some(StopReason { state: JobState::Cancelled, reason: reason.to_string() }));
        } else {
            drop(jobs);
            // Not running: report what the ledger has (or that it's unknown).
            let row = t.ledger.lock().unwrap().get(job_id, generation).ok().flatten();
            match row {
                Some(r) => {
                    t.hub.send(self.row_state_frame(&r));
                }
                None => {
                    t.hub.send(self.state_frame(job_id, generation, JobState::Unknown, Some("not_on_node"), None));
                }
            }
        }
    }
}
