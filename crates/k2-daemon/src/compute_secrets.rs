//! Compute secret-env (prd-compute-secret-env-v1): a vault secret reaches a
//! compute job as an env var, sealed to the node at spawn time.
//!
//! This module is the controller's side of it:
//! - `compute:` vault consumers as people write them (`compute:<ws>/<node>`,
//!   `compute:<node>`, `*`) and as they're shown (`compute:<ws>/<node name>`).

use k2_core::compute::store;
use k2_vault::{Consumer, VaultError};

/// `compute:[<ws>/]<node|*>` → the vault consumer
/// `compute:<workspace id>/<node id or *>`. `caller_ws` is the caller's own
/// workspace, used when no workspace is named.
pub fn consumer(spec: &str, caller_ws: Option<&str>) -> Result<Consumer, VaultError> {
    let bad = |m: &str| VaultError::bad_request("bad_consumer", format!("{m} (use compute:<workspace>/<node>, or compute:<workspace>/* for every node this server owns)"));
    let (ws_tok, node_tok) = match spec.split_once('/') {
        Some((w, n)) => (Some(w), n),
        None => (None, spec),
    };
    let ws_id = match ws_tok {
        Some(w) if !w.is_empty() => crate::vault::workspace_id(w).ok_or_else(|| VaultError::not_found(format!("no workspace named {w}")))?,
        Some(_) => return Err(bad("empty workspace")),
        None => caller_ws.map(str::to_string).ok_or_else(|| bad("name the workspace"))?,
    };
    if node_tok.is_empty() {
        return Err(bad("name a node or *"));
    }
    let node_id = if node_tok == "*" {
        "*".to_string()
    } else {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::find_node(&conn, node_tok)
            .filter(|n| n.state != "revoked")
            .map(|n| n.id)
            .ok_or_else(|| VaultError::not_found(format!("no compute node '{node_tok}' (k2 compute nodes)")))?
    };
    let c = Consumer::compute(&ws_id, &node_id);
    if c.compute_parts().is_none() {
        return Err(bad("that workspace or node can't be named here"));
    }
    Ok(c)
}

/// How people see a compute consumer: `compute:<ws handle>/<node name>`.
pub fn label(c: &Consumer) -> String {
    let Some((ws, node)) = c.compute_parts() else { return c.label() };
    let node_name = if node == "*" {
        "*".to_string()
    } else {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::node_by_id(&conn, node).map(|n| n.name).unwrap_or_else(|| format!("{node} (removed)"))
    };
    format!("compute:{}/{node_name}", crate::vault::workspace_label(ws))
}
