//! Rows of the route policy table (`routes::route_policy::ROUTES`):
//! the `/cli/vault` routes, sorted by path.
//! The floors and what a row means are documented in `route_policy.rs`.

use super::Floor::{Admin, Member, Owner};
use super::{both, get, post, split, Route};

pub(super) const ROWS: &[Route] = &[
    // K2 vault (prd-k2-vault-v1 §22.3, KV14). Passports reach the Member
    // rows through is_agent_verb and are checked in the handler; confirm,
    // reveal, recovery and key operations are never agent verbs.
    both("/cli/vault/accept-from", Member),
    post("/cli/vault/agent-policy", Member),
    post("/cli/vault/ask", Member),
    get("/cli/vault/asks", Member),
    get("/cli/vault/audit", Admin),
    post("/cli/vault/backup", Owner),
    post("/cli/vault/backup/recipient", Owner),
    post("/cli/vault/carry/export", Admin),
    post("/cli/vault/carry/import", Admin),
    post("/cli/vault/confirm-password/set", Owner),
    post("/cli/vault/confirm/finish", Admin),
    post("/cli/vault/confirm/start", Admin),
    post("/cli/vault/destroy", Member),
    post("/cli/vault/disable", Member),
    get("/cli/vault/doctor", Member),
    post("/cli/vault/enable", Member),
    post("/cli/vault/env-file", Member),
    post("/cli/vault/fetch", Member),
    post("/cli/vault/file", Member),
    post("/cli/vault/get", Member),
    post("/cli/vault/grant", Member),
    post("/cli/vault/import", Member),
    get("/cli/vault/incoming", Member),
    post("/cli/vault/incoming/decide", Member),
    post("/cli/vault/kek/rotate", Owner),
    get("/cli/vault/list", Member),
    // prd-vault-gaps-v1 item 4: manual placement records.
    // Open a vault that failed to open at boot (may prompt once).
    post("/cli/vault/open", Admin),
    post("/cli/vault/placement/add", Member),
    post("/cli/vault/placement/done", Member),
    post("/cli/vault/placement/remove", Member),
    get("/cli/vault/placements", Member),
    post("/cli/vault/policy", Member),
    get("/cli/vault/presets", Member),
    post("/cli/vault/proxy-url", Member),
    get("/cli/vault/public-key", Member),
    get("/cli/vault/recipient", Admin),
    post("/cli/vault/recovery/confirm", Owner),
    post("/cli/vault/recovery/rotate", Owner),
    post("/cli/vault/recovery/show-once", Owner),
    post("/cli/vault/rekey", Owner),
    get("/cli/vault/requests", Member),
    post("/cli/vault/restore", Owner),
    post("/cli/vault/reveal", Admin),
    post("/cli/vault/revoke", Member),
    post("/cli/vault/rollback", Owner),
    post("/cli/vault/rotate", Member),
    // `k2 vault run`: the handler serves agent passports only.
    post("/cli/vault/run", Member),
    get("/cli/vault/run-helper", Member),
    post("/cli/vault/send", Member),
    // 0.46.11: standing send grants. Reading is the agent's own or a
    // person's; adding or removing one is an Owner's or Admin's (with the
    // vault confirm to add). Passports reach GET only (the handler refuses
    // their POST).
    split("/cli/vault/send-allow", Member, Admin),
    get("/cli/vault/sent", Member),
    both("/cli/vault/service", Member),
    post("/cli/vault/set", Member),
    get("/cli/vault/show", Member),
    get("/cli/vault/status", Member),
    post("/cli/vault/sync", Member),
    // prd-vault-gaps-v1 item 4: a root unit's credential is Owner/Admin +
    // a vault confirm (passports refused in the handler, KV14 DENY);
    // (manual placement records, above, are bookkeeping: agents keep
    // their own workspace's).
    post("/cli/vault/systemd-cred", Admin),
    post("/cli/vault/systemd-cred/remove", Admin),
    post("/cli/vault/unsend", Member),
];
