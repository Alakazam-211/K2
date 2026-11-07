//! Protocol 9 on the node (prd-ci-on-compute-for-clients-v1, slice C0):
//! the signed self-statement and who gets it, the taint mark (V10), and
//! receipt v2's run facts. Every piece is for a session that speaks
//! protocol 9 only ([`Tenant::speaks_ci`]); a protocol-8 server sees
//! 0.46.9's node.

use std::sync::Arc;

use k2_node_proto::ci::{self, Isolation, RunFacts, Signed};
use k2_node_proto::frames::{Frame, Plan, PROTOCOL, RECEIPT_V2};

use super::{Node, Tenant, NODE_VERSION};
use crate::wall::Wall;

/// Refusal code: the plan asks for a receipt version this node doesn't
/// make (only an absent `receipt_v` or 2 are known).
pub const RECEIPT_V_UNKNOWN: &str = "receipt_v_unknown";

impl Node {
    /// Re-sign the self-statement if a fact changed; store it and send it
    /// to every protocol-9 session. True when it changed.
    pub fn refresh_statement(&self) -> bool {
        let state = self.layout().state();
        let node_file = crate::statement::read_taint(&state.join(crate::statement::TAINT_FILE));
        let root = *self.root_taint.lock().unwrap();
        let taint = crate::statement::taint_of(&node_file, root, &self.key_meta);
        let host = self.host.lock().unwrap().clone();
        let key_mode = {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(self.layout().key()).map(|m| m.mode()).unwrap_or(0o600)
        };
        let machine = crate::statement::machine_id();
        let facts = crate::statement::Facts {
            key: &self.key,
            meta: &self.key_meta,
            wall: crate::statement::wire_wall(&self.wall),
            dev: self.opts.dev,
            taint,
            key_owner: &self.opts.user,
            key_mode,
            version: NODE_VERSION,
            commit: crate::build_commit(),
            protocol: PROTOCOL,
            os: crate::sysinfo::os_label(),
            arch: crate::sysinfo::arch_label(),
            host: &host,
            ptrace_scope: crate::statement::ptrace_scope(),
            machine_id: machine.as_deref(),
        };
        let next = crate::statement::build(&facts, self.now());
        let signed = {
            let mut cur = self.statement.lock().unwrap();
            match cur.update(next, &self.key) {
                Ok(true) => cur.signed.clone(),
                Ok(false) => None,
                Err(e) => {
                    crate::nlog!("self-statement: {e}");
                    None
                }
            }
        };
        let Some(signed) = signed else { return false };
        if let Err(e) = crate::statement::store(&state, &signed) {
            crate::nlog!("self-statement: keeping it: {e}");
        }
        {
            let mut s = self.status.lock().unwrap();
            s.statement_digest = Some(signed.digest());
            s.taint = Some(taint_label(taint).to_string());
            s.key_epoch = Some(self.key_meta.key_epoch);
        }
        for t in self.tenants() {
            if t.speaks_ci() {
                t.hub.send(Frame::NodeStatement(signed.clone()));
            }
        }
        true
    }

    /// The signed self-statement in force.
    pub fn statement_signed(&self) -> Option<Signed> {
        self.statement.lock().unwrap().signed.clone()
    }

    /// V10: a job is starting under the wall `none` (`why`): the node key
    /// is tainted from now on. Marked in the node's own file at once, then
    /// in root's (in the background; a refusal leaves the node's mark to be
    /// handed over at the next start), and the statement says so.
    pub(super) fn mark_unwalled_start(self: &Arc<Self>, why: &str) {
        let path = self.layout().state().join(crate::statement::TAINT_FILE);
        let fp = self.key_meta.node_fp.clone();
        match crate::statement::mark_taint(&path, &fp, why, self.now()) {
            Ok(false) => return,
            Ok(true) => crate::nlog!("node key {}… is tainted: a job started without a job wall ({why}); a CI lane takes it only after a re-key", &fp[..12]),
            Err(e) => crate::nlog!("node key taint mark: {e}"),
        }
        self.refresh_statement();
        if self.opts.dev {
            return;
        }
        let me = self.clone();
        let why = why.to_string();
        tokio::spawn(async move {
            let sock = me.opts.admin_socket.clone();
            let op = crate::admin::jobs::JobOp::KeyTaint { node_fp: fp.clone(), mark: Some(why) };
            let r = tokio::task::spawn_blocking(move || crate::admin::server::job_request(&sock, &op, &[], std::time::Duration::from_secs(10))).await;
            if let Ok(Ok(crate::admin::jobs::JobAnswer::KeyTaint { tainted: true })) = r {
                *me.root_taint.lock().unwrap() = Some(true);
                if let Ok(mut f) = crate::statement::read_taint(&path) {
                    if let Some(m) = f.keys.get_mut(&fp) {
                        m.root_ack = true;
                    }
                    let _ = crate::statement::write_taint(&path, &f);
                }
            }
        });
    }

    /// Why a plan's `receipt_v` can't be honoured here, if it can't: only
    /// an absent one (v1) or 2, and 2 only from a protocol-9 session.
    pub(super) fn receipt_v_refusal(&self, t: &Tenant, plan: &Plan) -> Option<String> {
        match plan.receipt_v {
            None => None,
            Some(RECEIPT_V2) if t.speaks_ci() => None,
            Some(v) => Some(format!("this node makes receipt v1, or v2 for a protocol-9 server; the plan asks for v{v}")),
        }
    }

    /// How a job of `plan` is kept apart from other jobs, before any
    /// per-job uid (C2 sets that one when the executor gives it).
    pub fn base_isolation(&self, plan: &Plan) -> Isolation {
        if plan.exclusive {
            return Isolation::Exclusive;
        }
        match self.wall {
            Wall::JobUser { .. } => Isolation::TenantUid,
            Wall::Seatbelt | Wall::None { .. } => Isolation::SharedUid,
        }
    }

    /// Receipt v2's facts for a job of `t` with `plan`, as of now: the
    /// statement in force, the wall, the slot. `None` without a statement
    /// (signing it failed): the receipt then stays v1 and says nothing it
    /// can't back.
    pub fn run_facts(&self, t: &Tenant, isolation: Isolation, uid_index: Option<u32>) -> Option<RunFacts> {
        let digest = self.statement.lock().unwrap().digest()?;
        let f = RunFacts {
            isolation,
            uid_index,
            wall: crate::statement::wire_wall(&self.wall),
            tenant_slot: t.slot as u32,
            k2node_version: NODE_VERSION.to_string(),
            k2node_commit: crate::build_commit().map(str::to_string),
            key_epoch: self.key_meta.key_epoch,
            statement_digest: digest,
            // C1 (the owner's node roster) and C3 (lanes) fill these.
            roster_seq_seen: None,
            lane: None,
        };
        match f.check() {
            Ok(()) => Some(f),
            Err(e) => {
                crate::nlog!("receipt v2 facts: {e}");
                None
            }
        }
    }

    /// Record a running attempt's start-time facts (a plan with
    /// `receipt_v: 2` only).
    pub(super) fn set_ci_run(&self, t: &Tenant, job: &str, gen: u32, f: Option<RunFacts>) {
        let key = (t.slot, job.to_string(), gen);
        match f {
            Some(f) => {
                self.ci_runs.lock().unwrap().insert(key, f);
            }
            None => {
                self.ci_runs.lock().unwrap().remove(&key);
            }
        }
    }

    pub(super) fn ci_run(&self, t: &Tenant, job: &str, gen: u32) -> Option<RunFacts> {
        self.ci_runs.lock().unwrap().get(&(t.slot, job.to_string(), gen)).cloned()
    }

    pub(super) fn take_ci_run(&self, t: &Tenant, job: &str, gen: u32) -> Option<RunFacts> {
        self.ci_runs.lock().unwrap().remove(&(t.slot, job.to_string(), gen))
    }

    /// The statement a v2 receipt names, to ride beside it to the server
    /// (`ReceiptFrame.statement`). `None` for a v1 receipt or a statement
    /// this node no longer keeps.
    pub fn statement_for_receipt(&self, r: &k2_node_proto::frames::SignedReceipt) -> Option<Signed> {
        let body: serde_json::Value = serde_json::from_str(&r.body).ok()?;
        let digest = body.get("run")?.get("statement_digest")?.as_str()?;
        if let Some(cur) = self.statement_signed().filter(|s| s.digest() == digest) {
            return Some(cur);
        }
        crate::statement::lookup(&self.layout().state(), digest)
    }
}

/// The wire statement's taint, for display.
pub fn taint_label(t: ci::Taint) -> &'static str {
    match t {
        ci::Taint::Clean => "clean",
        ci::Taint::Tainted => "tainted",
        ci::Taint::Unknown => "unknown",
    }
}
