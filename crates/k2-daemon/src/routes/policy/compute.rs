//! Rows of the route policy table (`routes::route_policy::ROUTES`):
//! the `/cli/compute` routes, sorted by path.
//! The floors and what a row means are documented in `route_policy.rs`.

use super::Floor::{Admin, Member, Owner, Public};
use super::{get, post, Route};

pub(super) const ROWS: &[Route] = &[
    // K2 compute nodes (prd-k2-compute-nodes-v1 §15.1, CN18). attach and
    // enroll are the node's sockets: no HTTP credential, handlers fail
    // closed (CN11). Agent verbs are also `is_agent_verb` rows; owner/admin
    // verbs refuse passports in the handler (`owner_only`).
    // Node admin (prd-k2-node-admin-v1 §13.1). approve is Owner (Q2) and
    // needs a fresh vault confirm on top; passports are checked in the
    // handler (operate switch), never approve/deny.
    get("/cli/compute/admin/approval-key", Admin),
    post("/cli/compute/admin/approve", Owner),
    post("/cli/compute/admin/deny", Admin),
    post("/cli/compute/admin/propose", Admin),
    get("/cli/compute/admin/requests", Admin),
    post("/cli/compute/admin/withdraw", Admin),
    get("/cli/compute/agents", Admin),
    post("/cli/compute/agents/set", Admin),
    // A workspace's node list (0.46.8): anyone who may see its compute
    // reads it; the handler lets a person (owner, Admin) change it and an
    // agent passport only narrow its own (`compute_allow`).
    get("/cli/compute/allow-nodes", Member),
    post("/cli/compute/allow-nodes/set", Member),
    get("/cli/compute/attach", Public),
    // Job caches on the nodes (`--cache NAME`, 0.46.6): list and clear,
    // owner/Admin (or an agent under "Allow managing compute nodes").
    get("/cli/compute/cache", Admin),
    post("/cli/compute/cache/clear", Admin),
    get("/cli/compute/enroll", Public),
    get("/cli/compute/events", Admin),
    // Git access for jobs (0.46.9, GH#78): anyone who may see the
    // workspace's compute reads it; the handler lets a person grant, an
    // agent revoke its own; setup is the owner's or an Admin's.
    get("/cli/compute/git-access", Member),
    post("/cli/compute/git-access/grant", Member),
    post("/cli/compute/git-access/revoke", Member),
    post("/cli/compute/git-access/setup", Admin),
    get("/cli/compute/grants", Member),
    post("/cli/compute/grants/revoke", Admin),
    post("/cli/compute/grants/set", Admin),
    get("/cli/compute/jobs", Member),
    get("/cli/compute/jobs/artifact", Member),
    post("/cli/compute/jobs/cancel", Member),
    get("/cli/compute/jobs/get", Member),
    get("/cli/compute/jobs/logs", Member),
    post("/cli/compute/jobs/move", Member),
    get("/cli/compute/jobs/receipt", Member),
    post("/cli/compute/jobs/retry", Member),
    post("/cli/compute/jobs/tier", Member),
    // Node rosters, lanes, seats (0.46.10 C1/C3a, `compute_trust`): an
    // operator files (owner, Admin, or an agent with the operate switch);
    // a person decides (trust/approve, roster/key, consent on: the handler
    // refuses passports and needs a fresh confirm).
    get("/cli/compute/lanes", Admin),
    post("/cli/compute/lanes/set", Admin),
    post("/cli/compute/local/disable", Owner),
    post("/cli/compute/local/drain", Owner),
    post("/cli/compute/local/enable", Owner),
    post("/cli/compute/local/pause", Owner),
    post("/cli/compute/local/policy", Owner),
    // The remote-admin toggles (prd-compute-update-continuity-v1 Q4).
    post("/cli/compute/local/remote-admin", Owner),
    post("/cli/compute/local/resume", Owner),
    get("/cli/compute/local/status", Member),
    get("/cli/compute/nodes", Member),
    post("/cli/compute/nodes/alerts", Admin),
    post("/cli/compute/nodes/confirm", Owner),
    post("/cli/compute/nodes/enroll-code", Owner),
    get("/cli/compute/nodes/health", Member),
    get("/cli/compute/nodes/info", Member),
    // Package requests (0.46.9, prd-compute-self-serve-environments §12.2):
    // members with a grant read and ask; the owner or an Admin, in person,
    // decide (the handler refuses passports); the policy is an operator's
    // (narrowing at once, widening held).
    get("/cli/compute/nodes/packages", Member),
    post("/cli/compute/nodes/packages/decide", Admin),
    post("/cli/compute/nodes/packages/policy", Member),
    post("/cli/compute/nodes/packages/request", Member),
    get("/cli/compute/nodes/packages/requests", Member),
    post("/cli/compute/nodes/pause", Admin),
    get("/cli/compute/nodes/policy", Admin),
    post("/cli/compute/nodes/policy/set", Admin),
    post("/cli/compute/nodes/remove", Owner),
    post("/cli/compute/nodes/rename", Admin),
    post("/cli/compute/nodes/retire", Owner),
    post("/cli/compute/nodes/rollback", Admin),
    // k2-node rollouts (prd-compute-update-continuity-v1 §5.10): the
    // handler's operator() gate, as for update.
    get("/cli/compute/nodes/rollout", Admin),
    post("/cli/compute/nodes/rollout/pause", Admin),
    post("/cli/compute/nodes/rollout/resume", Admin),
    post("/cli/compute/nodes/rollout/stop", Admin),
    post("/cli/compute/nodes/routes", Admin),
    // Node sharing (prd-compute-shared-queue-v1 §3, §9): the servers on a
    // node this server owns. Agents need "Allow managing compute nodes" or
    // the operate switch (checked in the handler); revoke is the owner's.
    get("/cli/compute/nodes/servers", Admin),
    post("/cli/compute/nodes/servers/add", Admin),
    post("/cli/compute/nodes/servers/limits", Admin),
    // Changing a node's owner server is the owner's alone (Q9), and held
    // for the owner's approval (prd-macos-tenant-compute-v1 §6).
    post("/cli/compute/nodes/servers/owner", Owner),
    post("/cli/compute/nodes/servers/pause", Admin),
    post("/cli/compute/nodes/servers/revoke", Owner),
    // A node that says this server owns it now is taken over only by the
    // owner, in person (prd-macos-tenant-compute-v1 MT36).
    post("/cli/compute/nodes/take-owner", Owner),
    post("/cli/compute/nodes/update", Admin),
    // Q5 (0.46.4): an operator turns the Linux job wall on (handler: operator()).
    post("/cli/compute/nodes/wall", Admin),
    get("/cli/compute/owners", Admin),
    post("/cli/compute/owners/consent", Admin),
    // 0.46.7 (CE-18): Batch jobs that install --needs on granted nodes.
    post("/cli/compute/prewarm", Member),
    // A node's queue, redacted per §7 (an agent verb: its own workspace).
    get("/cli/compute/queue", Member),
    get("/cli/compute/roster", Admin),
    post("/cli/compute/roster/key", Admin),
    post("/cli/compute/roster/propose", Admin),
    post("/cli/compute/roster/publish", Admin),
    post("/cli/compute/run", Member),
    get("/cli/compute/seats", Admin),
    // Seat end / suspend / resume (11-07 trust review M7): the owner, in
    // person (the handler: owner_person + a fresh confirm bound to the seat).
    post("/cli/compute/seats/end", Admin),
    post("/cli/compute/seats/offer", Admin),
    post("/cli/compute/seats/request", Admin),
    post("/cli/compute/seats/resume", Admin),
    post("/cli/compute/seats/suspend", Admin),
    post("/cli/compute/trust/approve", Admin),
    post("/cli/compute/trust/deny", Admin),
    get("/cli/compute/trust/requests", Admin),
    post("/cli/compute/trust/withdraw", Admin),
    get("/cli/compute/usage", Admin),
];
