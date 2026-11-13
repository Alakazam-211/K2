//! Build statements on the node (prd-compute-build-attestations-v1 §4–§5,
//! slice S2).
//!
//! When a plan asks (`Plan.provenance = 1`, sent only to a node offering
//! `provenance-v1`, which this node offers to every protocol-10 server),
//! the node signs one in-toto statement per job in the same finish step
//! that signs the receipt, after the job's processes are gone and its
//! artifacts hashed. The job never sees the key or the statement.
//!
//! The statement is made from the signed receipt's own body
//! ([`k2_node_proto::provenance::Statement::for_receipt`]), the plan and a
//! few facts only the node has: the network the job started under, the
//! isolation for a v1 receipt, the owner it belongs to. Before it is kept,
//! the node opens it again with its own public key against the receipt
//! ([`k2_node_proto::provenance::Dsse::open_for_receipt`]), so a statement
//! that disagrees with its receipt is never written.
//!
//! No artifact, no statement. A statement that can't be made (no start
//! facts after a k2-node restart adopted the job, a payload over the cap)
//! leaves the receipt signed alone and a note in the job's results.

use k2_node_proto::ci::Isolation;
use k2_node_proto::frames::{Assign, Plan, Receipt, SignedReceipt, PROTOCOL};
use k2_node_proto::provenance::{Dsse, NodeFacts, Owner, Statement, PROVENANCE_V1};
use k2_node_proto::share::Network;

use super::{Node, Tenant, NODE_VERSION};

/// Refusal code: the plan asks for a build statement version this node
/// doesn't make, or its server can't take one (below protocol 10).
pub const PROVENANCE_V_UNKNOWN: &str = "provenance_v_unknown";

/// What a running attempt started under, kept until it ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProvStart {
    pub network: Network,
    pub isolation: Isolation,
}

/// What `finish_job` gets back.
#[derive(Debug)]
pub enum Outcome {
    /// Signed, opened again and checked against its receipt.
    Signed(Dsse),
    /// The plan didn't ask.
    NotAsked,
    /// The job kept no artifact: no statement (BA-30).
    NoArtifacts,
    /// Asked for and due, but not made: the receipt goes alone.
    Failed(String),
}

impl Node {
    /// `provenance-v1` is offered to a server that speaks protocol 10.
    pub fn offers_provenance(&self, t: &Tenant) -> bool {
        t.speaks_gens()
    }

    /// A plan's `provenance` this node can't honour (an unknown version, or
    /// a session below protocol 10 that could never have been offered it).
    pub(super) fn provenance_refusal(&self, t: &Tenant, plan: &Plan) -> Option<String> {
        match plan.provenance {
            None => None,
            Some(PROVENANCE_V1) if self.offers_provenance(t) => None,
            Some(v) => Some(format!("this node signs build statement v1 for a protocol-10 server; the plan asks for v{v}")),
        }
    }

    /// The attempt is about to start, under `network`.
    pub(super) fn set_prov_start(&self, t: &Tenant, job: &str, gen: u32, plan: &Plan, network: Network) {
        if plan.provenance.is_none() {
            return;
        }
        let s = ProvStart { network, isolation: self.base_isolation(plan) };
        self.prov_starts.lock().unwrap().insert((t.slot, job.to_string(), gen), s);
    }

    /// C2: the attempt got a uid of its own.
    pub(super) fn set_prov_start_job_uid(&self, t: &Tenant, job: &str, gen: u32) {
        if let Some(s) = self.prov_starts.lock().unwrap().get_mut(&(t.slot, job.to_string(), gen)) {
            s.isolation = Isolation::JobUid;
        }
    }

    /// The facts only the node has, for `t`'s job that started under `start`.
    pub fn provenance_facts(&self, t: &Tenant, start: ProvStart) -> Result<NodeFacts, String> {
        let roster = self.roster_held();
        let owner = match &roster {
            Some(r) => Owner::RosterKey(r.roster.key_fp()?),
            None => {
                let fp = self.owner().pin.lock().unwrap().as_ref().map(|p| p.controller_fp.clone()).ok_or("no owner server pinned")?;
                if !k2_node_proto::ci::is_fp(&fp) {
                    return Err("the owner pin is not a fingerprint".into());
                }
                Owner::Server(fp)
            }
        };
        let node_fp = self.key.fingerprint();
        Ok(NodeFacts {
            owner,
            network: start.network,
            isolation: start.isolation,
            wall: crate::statement::wire_wall(&self.wall),
            tenant_slot: t.slot as u32,
            lane: self.lane_of(t).map(|l| l.lane),
            k2node_version: NODE_VERSION.to_string(),
            k2node_commit: crate::build_commit().map(str::to_string),
            protocol: PROTOCOL,
            key_epoch: self.key_meta.key_epoch,
            statement_digest: self.statement.lock().unwrap().digest(),
            node_name: roster.as_ref().and_then(|r| r.roster.entry(&node_fp)).map(|e| e.name.clone()),
        })
    }

    /// The statement for an ending attempt, beside its signed receipt.
    /// Always forgets the attempt's start facts.
    pub(super) fn sign_provenance(&self, t: &Tenant, a: &Assign, signed: Option<&SignedReceipt>) -> Outcome {
        let start = self.prov_starts.lock().unwrap().remove(&(t.slot, a.fence.job_id.clone(), a.fence.generation));
        if a.plan.provenance != Some(PROVENANCE_V1) {
            return Outcome::NotAsked;
        }
        let Some(signed) = signed else {
            return Outcome::Failed("the receipt wasn't signed".into());
        };
        let no_artifacts = match serde_json::from_str::<Receipt>(&signed.body) {
            Ok(r) => r.artifacts.is_empty(),
            Err(e) => return Outcome::Failed(format!("the receipt body: {e}")),
        };
        if no_artifacts {
            return Outcome::NoArtifacts;
        }
        let Some(start) = start else {
            return Outcome::Failed("this k2-node process didn't start the job (it was adopted after an update), so it can't say how it ran".into());
        };
        let facts = match self.provenance_facts(t, start) {
            Ok(f) => f,
            Err(e) => return Outcome::Failed(e),
        };
        let st = match Statement::for_receipt(signed, &a.plan, &facts) {
            Ok(Some(st)) => st,
            Ok(None) => return Outcome::NoArtifacts,
            Err(e) => return Outcome::Failed(e),
        };
        let env = match self.key.sign_statement(&st) {
            Ok(e) => e,
            Err(e) => return Outcome::Failed(e),
        };
        // Both or neither: what is kept is what a reader will accept.
        match env.open_for_receipt(self.key.spki_der(), signed) {
            Ok(_) => Outcome::Signed(env),
            Err(e) => Outcome::Failed(format!("the signed statement didn't check against its receipt: {e}")),
        }
    }
}
