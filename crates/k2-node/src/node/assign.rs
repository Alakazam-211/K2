//! `assign` and `cancel` from a server, and the state frames that answer
//! them. Everything acts on the session's tenant: its pin (the plan must
//! name this node as that server knows it), its ledger, and its slot in
//! the running-jobs map (V48).
//!
//! **Solo** (one active server, or none in `servers.toml`): an assign
//! starts or is refused, exactly as in 0.46.2.
//!
//! **Shared** (two or more active servers; prd-compute-shared-queue-v1 §7):
//! - a server that speaks protocol ≥ 3 sends jobs *into* the node queue:
//!   share and node refusals answer at once (never written to the ledger),
//!   anything else becomes a `queued/node_queued` ledger row that the queue
//!   ([`crate::fairq`]) starts in fair order;
//! - an old (protocol 2) server never queues: its assign starts only when
//!   no other server has queued work and it fits now, otherwise it's
//!   refused `fair_share_wait` and the server retries (V70 b);
//! - `cancel` with `withdraw` or `if_queued` never touches a started job
//!   (V57); a withdrawn row ends `cancelled/withdrawn` (V46);
//! - after each attach, queued rows the server's `Welcome.resume` doesn't
//!   list end `cancelled/superseded`, and only listed ones may start (V47).

use std::sync::Arc;

use k2_node_proto::crypto;
use k2_node_proto::frames::{self, Assign, Fence, Frame, JobState, Receipt, ReceiptLog, ResumeHint, SignedReceipt, StateUpdate};
use k2_node_proto::refusal::{self, JobAsk, NodeView};
use k2_node_proto::share::{codes, JobClass, ServerStatus, OWNER_SLOT};
use tokio::sync::watch;

use super::{Node, RunningJob, StopReason, Tenant};

/// One running attempt, as the queue sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningSnap {
    pub slot: u8,
    pub job_id: String,
    pub generation: u32,
}

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
    /// generation))` when the node took it (started or queued: the session
    /// tracks its state and logs).
    pub fn on_assign(self: &Arc<Self>, t: &Arc<Tenant>, a: Assign) -> Option<(String, u32)> {
        let job = a.fence.job_id.clone();
        let gen = a.fence.generation;
        // Before anything else (no row, no folder): the id names folders
        // the node cleans with its own rights (review A-disk 2).
        if !a.job_id_ok() {
            crate::nlog!("assign refused: job id {:?} isn't 1-64 of A-Z a-z 0-9 _ -", a.fence.job_id.chars().take(80).collect::<String>());
            t.hub.send(self.state_frame(&job, gen, JobState::Failed, Some("bad_job_id"), Some("a job id is 1-64 of A-Z a-z 0-9 _ -")));
            return None;
        }
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
        let now = self.now();
        if self.is_stopping() {
            t.hub.send(self.state_frame(&job, gen, JobState::Queued, Some(refusal::LOCAL_STOPPED), Some("k2-node is stopping")));
            return None;
        }
        // Only the owner, on a node nobody shares, takes today's path. Any
        // other server always goes through its share (a paused, expired or
        // revoked share must never fall back to "solo").
        if t.slot != OWNER_SLOT || self.fairq.shared(now) {
            if self.fairq.protocol(t.slot) >= frames::PROTOCOL_SHARED {
                return self.enqueue(t, a);
            }
            if t.slot != OWNER_SLOT {
                // A second server must speak protocol 3 (its attach is
                // refused `server_too_old`; this is the backstop).
                t.hub.send(self.state_frame(&job, gen, JobState::Queued, Some(codes::SERVER_TOO_OLD), Some("update this server to 0.46.3 to use a shared node")));
                return None;
            }
            // An old server on a shared node (V70 b): it can't wait in the
            // queue, so it runs only when nobody else has queued work.
            let others_waiting = self.tenants().iter().filter(|x| x.slot != t.slot).any(|x| x.ledger.lock().unwrap().queued().is_ok_and(|q| !q.is_empty()));
            if others_waiting {
                t.hub.send(self.state_frame(&job, gen, JobState::Queued, Some(codes::FAIR_SHARE_WAIT), Some("another server's jobs are queued on this shared node; update this server to 0.46.3 to queue")));
                return None;
            }
        }
        if let Some(r) = self.assign_refusal(t, &a) {
            t.hub.send(self.state_frame(&job, gen, JobState::Queued, Some(&r.code), Some(&r.message)));
            return None;
        }
        if let Err(e) = t.ledger.lock().unwrap().insert(&a, self.opts.clock.as_ref()) {
            t.hub.send(self.state_frame(&job, gen, JobState::Failed, Some("ledger_error"), Some(&e)));
            return None;
        }
        self.launch(t, a);
        Some((job, gen))
    }

    /// Start an attempt whose ledger row is in place: the running map,
    /// the queue's accounting, the job task.
    fn launch(self: &Arc<Self>, t: &Arc<Tenant>, a: Assign) {
        let (stop_tx, stop_rx) = watch::channel(None);
        self.jobs.lock().unwrap().insert(
            (t.slot, a.fence.job_id.clone()),
            RunningJob {
                generation: a.fence.generation,
                cpu_millis: a.plan.limits.cpu_millis,
                mem_bytes: a.plan.limits.mem_bytes,
                exclusive: a.plan.exclusive,
                stop: stop_tx,
                seq: self.job_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            },
        );
        let caps = self.queue_caps();
        self.fairq.started(t.slot, &a, self.now(), &caps);
        self.jobs_changed();
        let (node, tenant) = (self.clone(), t.clone());
        tokio::spawn(async move { node.run_job(tenant, a, stop_rx).await });
    }

    pub(super) fn assign_refusal(&self, t: &Tenant, a: &Assign) -> Option<k2_node_proto::frames::Refusal> {
        let l = self.local.lock().unwrap().clone();
        let (cap_cpu, cap_mem, par) = self.caps(t, &l);
        let (used_cpu, used_mem, running, excl) = self.used(cap_cpu, cap_mem, par);
        let disk_free = self.disk_free_bytes();
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

    // ── the node queue (shared mode) ────────────────────────────────

    /// The node's capacity and node-wide block for the queue: the
    /// policy's caps narrowed by the persisted narrowing (a session's
    /// narrowing narrows only that server's view), and the first reason
    /// nothing may start (control, window, lock, disk, network policy, an
    /// install).
    pub(crate) fn queue_caps(&self) -> k2_node_proto::queue::Caps {
        let l = self.local.lock().unwrap().clone();
        let persisted = self.persisted_narrow.lock().unwrap().clone();
        let mut cpu = l.policy.cpu_cap_millis(self.facts.cores);
        let mut mem = l.policy.mem_cap_bytes;
        let mut par = l.policy.max_parallel;
        if let Some(c) = persisted.cpu_millis {
            cpu = cpu.min(c);
        }
        if let Some(m) = persisted.mem_bytes {
            mem = mem.min(m);
        }
        if let Some(p) = persisted.max_parallel {
            par = par.min(p.max(1));
        }
        let disk_free = self.disk_free_bytes();
        let tools = self.tools.lock().unwrap().clone();
        let none: [String; 0] = [];
        let view = NodeView {
            control: self.effective_control(&l),
            unavailable: &l.unavailable,
            foreign_lock: l.locks.holding.as_deref(),
            disk_free_bytes: disk_free,
            disk_floor_bytes: l.policy.disk_floor_bytes,
            workspaces_allow: &none,
            tools: &tools,
            running: 0,
            parallel_max: par,
            exclusive_running: false,
            free_cpu_millis: cpu,
            free_mem_bytes: mem,
            network_none: l.policy.network == "none",
            network_none_enforced: self.wall.network_none(),
        };
        let mut blocked = refusal::node_refusal(&view).map(|r| r.code);
        if self.is_stopping() {
            blocked = Some(refusal::LOCAL_STOPPED.into());
        }
        if blocked.is_none() && crate::toolchain::installing(&self.toolchains().root) {
            blocked = Some(codes::INSTALLING_TOOLS.into());
        }
        k2_node_proto::queue::Caps { cpu_millis: cpu, mem_bytes: mem, parallel_max: par, blocked }
    }

    /// Every running attempt (all tenants).
    pub(crate) fn running_snaps(&self) -> Vec<RunningSnap> {
        self.jobs.lock().unwrap().iter().map(|((slot, id), j)| RunningSnap { slot: *slot, job_id: id.clone(), generation: j.generation }).collect()
    }

    /// Shared mode, protocol ≥ 3: refuse now, or write the job into the
    /// queue (it may start at once: the queue decides).
    fn enqueue(self: &Arc<Self>, t: &Arc<Tenant>, a: Assign) -> Option<(String, u32)> {
        let job = a.fence.job_id.clone();
        let gen = a.fence.generation;
        if let Some((code, msg)) = self.queue_refusal(t, &a) {
            t.hub.send(self.state_frame(&job, gen, JobState::Queued, Some(code), Some(&msg)));
            return None;
        }
        if let Err(e) = t.ledger.lock().unwrap().insert_queued(&a, self.opts.clock.as_ref()) {
            t.hub.send(self.state_frame(&job, gen, JobState::Failed, Some("ledger_error"), Some(&e)));
            return None;
        }
        let epoch = *t.hub.connected.borrow();
        self.fairq.queued(t.slot, &a, epoch, self.now());
        self.fairq.kick.notify_one();
        Some((job, gen))
    }

    /// Why a job can't even wait here: share caps and classes, the
    /// tenant's queue, node facts no wait fixes. `None` = it may queue.
    fn queue_refusal(&self, t: &Tenant, a: &Assign) -> Option<(&'static str, String)> {
        let Some(share) = self.fairq.share(t.slot, self) else {
            return Some((codes::SERVER_NOT_APPROVED, "this node has no approved share for this server".into()));
        };
        match share.status {
            ServerStatus::Active | ServerStatus::Paused => {}
            ServerStatus::Expired => return Some((codes::SHARE_EXPIRED, "this server's share of the node has ended".into())),
            ServerStatus::Revoked => return Some((codes::SHARE_REVOKED, "this server's share of the node was revoked".into())),
            ServerStatus::Pending | ServerStatus::Refused => return Some((codes::SERVER_NOT_APPROVED, "this server isn't active on the node".into())),
        }
        let l = self.local.lock().unwrap().clone();
        // The owner's workspace allowlist applies to the owner only (V65).
        if t.slot == OWNER_SLOT && !l.policy.workspaces.is_empty() && !l.policy.workspaces.contains(&a.plan.workspace_id) {
            return Some((refusal::WORKSPACE_REFUSED, "the machine's owner doesn't allow this workspace".into()));
        }
        if l.policy.network == "none" && !self.wall.network_none() {
            return Some((refusal::NETWORK_NONE_NOT_ENFORCED, refusal::NETWORK_NONE_MESSAGE.into()));
        }
        if a.plan.src.is_some() && !self.tools.lock().unwrap().contains_key("git") {
            return Some((refusal::TOOL_MISSING, "git is not installed on the node".into()));
        }
        let lim = &share.limits;
        let class = a.plan.class.unwrap_or_default();
        if !lim.allows_class(class) {
            return Some(if class == JobClass::Gate {
                (codes::GATE_NOT_ALLOWED, "this server's share doesn't allow gate jobs".into())
            } else {
                (codes::EXCEEDS_SHARE_CAPS, format!("this server's share allows up to {} jobs", lim.top_class.as_str()))
            });
        }
        if a.plan.exclusive && !lim.exclusive {
            return Some((codes::EXCLUSIVE_NOT_ALLOWED, "this server's share doesn't allow exclusive jobs".into()));
        }
        let cpu = a.plan.limits.cpu_millis;
        let mem = a.plan.limits.mem_bytes;
        if cpu.is_some_and(|c| c > u64::from(lim.max_cpus) * 1000)
            || mem.is_some_and(|m| m > u64::from(lim.max_mem_gb) << 30)
            || a.plan.limits.max_secs > lim.max_job_secs
        {
            return Some((
                codes::EXCEEDS_SHARE_CAPS,
                format!("over this server's share: {} CPUs, {} GB, {} s per job", lim.max_cpus, lim.max_mem_gb, lim.max_job_secs),
            ));
        }
        let caps = self.queue_caps();
        if cpu.is_some_and(|c| c > caps.cpu_millis) || mem.is_some_and(|m| m > caps.mem_bytes) {
            return Some((k2_node_proto::share::blocked::EXCEEDS_NODE_CAPS, "bigger than this node's caps".into()));
        }
        let queued = t.ledger.lock().unwrap().queued().map(|q| q.len()).unwrap_or(usize::MAX);
        if queued >= lim.max_queued as usize {
            return Some((codes::TENANT_QUEUE_FULL, format!("this server already has {queued} jobs queued here (its share allows {})", lim.max_queued)));
        }
        None
    }

    /// Start a queued row now (the queue chose it). False when it left
    /// the queue meanwhile (withdrawn, cancelled): then nothing starts.
    pub(crate) fn start_queued(self: &Arc<Self>, t: &Arc<Tenant>, r: &crate::ledger::Row) -> bool {
        match t.ledger.lock().unwrap().dequeue(&r.job_id, r.generation, self.opts.clock.as_ref()) {
            Ok(true) => {}
            Ok(false) => return false,
            Err(e) => {
                crate::nlog!("start queued {}: {e}", r.job_id);
                return false;
            }
        }
        let a = Assign {
            fence: Fence { job_id: r.job_id.clone(), attempt: r.attempt, generation: r.generation, plan_digest: r.plan_digest.clone() },
            plan: r.plan.clone(),
        };
        self.launch(t, a);
        true
    }

    /// End a still-queued row `cancelled/<reason>` with a receipt (it never
    /// ran), tell its server, forget it in the queue. False when it had
    /// started (or is gone): nothing changed.
    pub(crate) fn cancel_queued_row(&self, t: &Tenant, job_id: &str, generation: u32, reason: &str, detail: Option<&str>) -> bool {
        let now = self.now();
        let row = {
            let l = t.ledger.lock().unwrap();
            match l.cancel_queued(job_id, generation, reason, detail, now) {
                Ok(true) => {}
                Ok(false) => return false,
                Err(e) => {
                    crate::nlog!("cancel queued {job_id}: {e}");
                    return false;
                }
            }
            let Ok(Some(row)) = l.get(job_id, generation) else { return false };
            match self.unrun_receipt(t, &row) {
                Ok(r) => {
                    let _ = l.set_receipt(job_id, generation, &r);
                }
                Err(e) => crate::nlog!("receipt for {job_id}: {e}"),
            }
            l.get(job_id, generation).ok().flatten().unwrap_or(row)
        };
        self.fairq.forget(t.slot, job_id, generation);
        t.hub.send(self.row_state_frame(&row));
        t.ship.notify_one();
        self.fairq.kick.notify_one();
        true
    }

    /// The receipt of an attempt that never ran (no start, no exit, no log).
    fn unrun_receipt(&self, t: &Tenant, r: &crate::ledger::Row) -> Result<SignedReceipt, String> {
        let mut env_names: Vec<String> = r.plan.env.keys().cloned().collect();
        env_names.sort();
        let receipt = Receipt {
            v: 1,
            job_id: r.job_id.clone(),
            attempt: r.attempt,
            generation: r.generation,
            plan_digest: r.plan_digest.clone(),
            node_fp: self.key.fingerprint(),
            controller_fp: t.pin.lock().unwrap().as_ref().map(|p| p.controller_fp.clone()).unwrap_or_default(),
            workspace_id: r.plan.workspace_id.clone(),
            argv_sha256: frames::argv_sha256(&r.plan.argv),
            env_names,
            src: None,
            tools: self.tools.lock().unwrap().clone(),
            boot_id: self.opts.clock.boot_id(),
            started_at: None,
            ended_at: r.ended_at.unwrap_or_else(|| self.now()),
            state: r.state,
            exit: None,
            log: ReceiptLog { sha256: crypto::sha256_hex(b""), bytes: 0, truncated: false },
            artifacts: Vec::new(),
        };
        SignedReceipt::sign(&self.key, &receipt)
    }

    /// After `t`'s server attached (call right after its hub attached,
    /// with its `Welcome.resume`): queued rows it still lists may start in
    /// this session; the rest end `cancelled/superseded` at once, so a job
    /// the server re-placed elsewhere while the node was away never runs
    /// here too (V47).
    pub fn queue_attach(&self, t: &Tenant, resume: &[ResumeHint]) {
        let epoch = *t.hub.connected.borrow();
        let rows = t.ledger.lock().unwrap().queued().unwrap_or_default();
        for r in rows {
            if resume.iter().any(|h| h.job_id == r.job_id && h.generation == r.generation) {
                self.fairq.gate(t.slot, &r.job_id, r.generation, epoch);
            } else {
                self.cancel_queued_row(t, &r.job_id, r.generation, codes::SUPERSEDED, Some("the server no longer lists this job"));
            }
        }
        self.fairq.resend(t.slot);
        self.fairq.kick.notify_one();
    }

    /// End every queued job of `t` (its share was revoked or expired, or
    /// it left the node). Running jobs aren't touched.
    pub fn queue_cancel_tenant(&self, t: &Tenant, reason: &str) -> usize {
        let rows = t.ledger.lock().unwrap().queued().unwrap_or_default();
        rows.iter().filter(|r| self.cancel_queued_row(t, &r.job_id, r.generation, reason, None)).count()
    }

    /// `cancel` from `t`'s server: only ever its own jobs. `withdraw` and
    /// `if_queued` end a job only while it's still queued; a started one is
    /// left alone and its real state is the answer (V57).
    pub fn cancel(self: &Arc<Self>, t: &Arc<Tenant>, job_id: &str, generation: u32, reason: &str) {
        let only_queued = frames::cancel_only_if_not_started(reason);
        if !only_queued {
            let detail = "its process had already ended and its end was never recorded; recorded now";
            if self.finish_if_orphan(t, job_id, generation, JobState::Cancelled, reason, detail) {
                return;
            }
        }
        let jobs = self.jobs.lock().unwrap();
        if let Some(j) = jobs.get(&(t.slot, job_id.to_string())).filter(|j| j.generation == generation) {
            if only_queued {
                drop(jobs);
                if let Ok(Some(r)) = t.ledger.lock().unwrap().get(job_id, generation) {
                    t.hub.send(self.row_state_frame(&r));
                }
                return;
            }
            j.stop.send_replace(Some(StopReason { state: JobState::Cancelled, reason: reason.to_string() }));
            return;
        }
        drop(jobs);
        // Not running: a queued row ends now; anything else reports what the
        // ledger has (or that it's unknown).
        let row = t.ledger.lock().unwrap().get(job_id, generation).ok().flatten();
        match row {
            Some(r) if r.state == JobState::Queued => {
                let why = match reason {
                    frames::CANCEL_WITHDRAW => codes::WITHDRAWN,
                    frames::CANCEL_IF_QUEUED => "cancelled",
                    other => other,
                };
                if !self.cancel_queued_row(t, job_id, generation, why, None) {
                    // It started between the read and the cancel: its real state.
                    if let Ok(Some(r)) = t.ledger.lock().unwrap().get(job_id, generation) {
                        t.hub.send(self.row_state_frame(&r));
                    }
                }
            }
            Some(r) => {
                t.hub.send(self.row_state_frame(&r));
            }
            None => {
                t.hub.send(self.state_frame(job_id, generation, JobState::Unknown, Some("not_on_node"), None));
            }
        }
    }

    /// An attempt that started (`running`/`finishing`, or a start time)
    /// whose row is still open but that no task runs: its runner ended
    /// without its end landing (a full disk before 0.46.3, a give-up after
    /// the finish retries). Such a row held a slot and showed `running`
    /// forever (k2n4, 2026-10-21). If `job_id` is one, record its end now
    /// (a placeholder holds its place so nothing else finishes it) and
    /// return true. A queued or just-dequeued row is never one.
    pub(crate) fn finish_if_orphan(self: &Arc<Self>, t: &Arc<Tenant>, job_id: &str, generation: u32, state: JobState, reason: &str, detail: &str) -> bool {
        let row = match t.ledger.lock().unwrap().get(job_id, generation) {
            Ok(Some(r)) => r,
            _ => return false,
        };
        let started = row.started_at.is_some() || matches!(row.state, JobState::Running | JobState::Finishing);
        if row.state.is_terminal() || row.state == JobState::Queued || !started {
            return false;
        }
        {
            let mut jobs = self.jobs.lock().unwrap();
            if jobs.contains_key(&(t.slot, job_id.to_string())) {
                return false;
            }
            let (stop, _) = watch::channel(None);
            jobs.insert(
                (t.slot, job_id.to_string()),
                RunningJob {
                    generation,
                    cpu_millis: Some(0),
                    mem_bytes: Some(0),
                    exclusive: false,
                    stop,
                    seq: 0,
                },
            );
        }
        // Review B F2: the runner may have landed its end and left the map
        // between the read above and the check. Read again with the place
        // held: an end that landed stands (no second end, receipt or
        // journal line).
        let ended = t.ledger.lock().unwrap().get(job_id, generation).ok().flatten().is_none_or(|r| r.state.is_terminal());
        if ended {
            self.jobs.lock().unwrap().remove(&(t.slot, job_id.to_string()));
            return false;
        }
        crate::nlog!("job {job_id} gen {generation}: no process runs it but its row is {}; recording its end ({reason})", row.state.as_str());
        let (node, t, reason, detail) = (self.clone(), t.clone(), reason.to_string(), detail.to_string());
        tokio::spawn(async move {
            let a = Assign {
                fence: Fence { job_id: row.job_id.clone(), attempt: row.attempt, generation: row.generation, plan_digest: row.plan_digest.clone() },
                plan: row.plan.clone(),
            };
            let mut journal = crate::journal::JournalWriter::reopen(&t.paths.journal(&row.job_id, row.generation), row.plan.limits.log_cap_bytes).ok();
            if let Some(j) = journal.as_mut() {
                let _ = j.append(frames::LogStream::Sys, format!("[k2-node] {detail}\n").as_bytes());
            }
            let dirs = crate::runner::JobDirs::new(t.paths.job_dir(&row.job_id, row.generation));
            node.finish_job(&t, &a, Some(&dirs), state, Some(&reason), Some(&detail), row.src.clone(), None, journal, None, row.outputs.clone()).await;
        });
        true
    }

    /// Record the end of every orphan (see [`Self::finish_if_orphan`]) as
    /// `failed / node_lost_job`. Returns how many. The disk guard's loop
    /// calls this, so a lost job frees its slot without a cancel.
    pub fn reap_orphans(self: &Arc<Self>) -> usize {
        let mut n = 0;
        for t in self.tenants() {
            let open = t.ledger.lock().unwrap().non_terminal().unwrap_or_default();
            for r in open {
                if self.finish_if_orphan(&t, &r.job_id, r.generation, JobState::Failed, NODE_LOST_JOB, "its process had ended and its end was never recorded; recorded now") {
                    n += 1;
                }
            }
        }
        n
    }
}

/// An attempt whose runner ended without recording its end.
pub const NODE_LOST_JOB: &str = "node_lost_job";
