//! The unprivileged node as courier (prd-k2-node-admin-v1 §5): it passes
//! command bytes to the root executor and results back. It can't approve,
//! widen or forge anything. Read verbs and job housekeeping it answers
//! itself, after checking the same controller signature against its own
//! pin.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use k2_node_proto::admin::{self, code, AdminKeys, AdminRefusal, AdminResult, Level, Runs, SignedCommand};
use serde_json::Value;

use super::exec::{LocalRequest, LocalResponse};
use crate::node::Node;

/// Single-use nonces for node-answered verbs (in memory: these verbs
/// change nothing root-owned; a restart forgetting them is harmless).
#[derive(Default)]
pub struct NodeNonces(Mutex<HashMap<String, i64>>);

impl NodeNonces {
    fn take(&self, nonce: &str, exp: i64, now: i64) -> bool {
        let mut m = self.0.lock().unwrap();
        m.retain(|_, e| *e + admin::CLOCK_SKEW_SECS >= now);
        if m.contains_key(nonce) {
            return false;
        }
        m.insert(nonce.to_string(), exp);
        true
    }
}

/// The level the machine owner set (admin.toml is world-readable).
pub fn local_level(node: &Node) -> Level {
    super::config::read(&node.layout().config.join(super::config::ADMIN_FILE))
        .ok()
        .flatten()
        .map(|c| c.level)
        .unwrap_or(Level::Off)
}

/// Ask the executor (blocking work off the async runtime).
pub async fn ask(node: &Node, req: LocalRequest, payload: Option<Vec<u8>>, timeout: Duration) -> Result<LocalResponse, AdminRefusal> {
    let socket = node.opts.admin_socket.clone();
    tokio::task::spawn_blocking(move || super::server::request(&socket, &req, payload.as_deref(), timeout))
        .await
        .map_err(|e| AdminRefusal::new(code::EXECUTOR_UNREACHABLE, e.to_string()))?
}

fn as_result(r: Result<LocalResponse, AdminRefusal>, cmd_id: &str, digest: &str) -> AdminResult {
    match r {
        Ok(LocalResponse::Result(r)) => r,
        Ok(LocalResponse::Status(_)) => AdminResult::refused(cmd_id, digest, &AdminRefusal::new(code::FAILED, "executor answered status")),
        Err(e) => AdminResult::refused(cmd_id, digest, &e),
    }
}

/// One `admin` frame → one `admin_result`.
pub async fn handle(node: Arc<Node>, cmd: SignedCommand) -> AdminResult {
    let raw = match cmd.to_json() {
        Ok(r) => r,
        Err(e) => return AdminResult::refused("", "", &AdminRefusal::new(code::MALFORMED, e)),
    };
    let pin = node.pin.lock().unwrap().clone();
    let spki = match pin.as_ref().map(|p| p.controller_spki()) {
        Some(Ok(s)) => s,
        _ => return AdminResult::refused("", "", &AdminRefusal::new(code::NOT_CONFIGURED, "node has no controller pin")),
    };
    let o = match admin::open(raw.as_bytes(), &spki) {
        Ok(o) => o,
        Err(r) => return AdminResult::refused("", "", &r),
    };
    let (cmd_id, digest) = (o.cmd.cmd_id.clone(), o.digest());
    let Some(spec) = admin::verb(&o.cmd.verb) else {
        return AdminResult::refused(&cmd_id, &digest, &AdminRefusal::new(code::VERB_NOT_ALLOWED, "verb is not on this node's list"));
    };
    if spec.runs == Runs::Executor {
        // Root checks everything again, itself.
        let timeout = Duration::from_secs(if o.cmd.verb == "unit.restart" { 120 } else { 60 });
        return as_result(ask(&node, LocalRequest::Command { cmd }, None, timeout).await, &cmd_id, &digest);
    }
    let pin_fp = pin.map(|p| p.controller_fp).unwrap_or_default();
    let now = node.now();
    if let Err(r) = admin::check_target_and_time(&o.cmd, &node.key.fingerprint(), &pin_fp, now) {
        return AdminResult::refused(&cmd_id, &digest, &r);
    }
    if local_level(&node) < spec.min_level {
        return AdminResult::refused(&cmd_id, &digest, &AdminRefusal::new(code::REMOTE_ADMIN_OFF, format!("{} needs remote admin {}", spec.name, spec.min_level.as_str())));
    }
    if !node.admin_nonces.take(&o.cmd.nonce, o.cmd.exp, now) {
        return AdminResult::refused(&cmd_id, &digest, &AdminRefusal::new(code::REPLAYED, "this command already ran"));
    }
    match node_verb(&node, &o.cmd.verb, &o.cmd.args).await {
        Ok(v) => AdminResult::done(&cmd_id, &digest, v),
        Err(r) => AdminResult::refused(&cmd_id, &digest, &r),
    }
}

/// Verbs the node answers itself (read + job housekeeping).
async fn node_verb(node: &Arc<Node>, verb: &str, args: &Value) -> Result<Value, AdminRefusal> {
    match verb {
        _ => {
            let _ = (node, args);
            Err(AdminRefusal::new(code::VERB_NOT_ALLOWED, "verb is not built on this node yet"))
        }
    }
}

/// `update_offer` (S5). Until built: refused.
pub fn update_offer(node: &Arc<Node>, tx: &tokio::sync::mpsc::Sender<k2_node_proto::frames::Frame>, o: k2_node_proto::frames::UpdateOffer) {
    let f = k2_node_proto::frames::Frame::UpdateState(k2_node_proto::frames::UpdateState {
        transfer_id: Some(o.transfer_id),
        cmd_id: None,
        phase: k2_node_proto::admin::UpdatePhase::Refused,
        version: o.version,
        from: Some(crate::PRODUCT_VERSION.into()),
        code: Some(code::VERB_NOT_ALLOWED.into()),
        message: Some("self-update is not built on this node yet".into()),
        at: node.now(),
    });
    let _ = tx.try_send(f);
}

/// `update_chunk` (S5).
pub fn update_chunk(_node: &Arc<Node>, _tx: &tokio::sync::mpsc::Sender<k2_node_proto::frames::Frame>, _c: k2_node_proto::frames::UpdateChunk) {}

/// `admin_keys` → the executor.
pub async fn keys(node: Arc<Node>, k: AdminKeys) -> AdminResult {
    as_result(ask(&node, LocalRequest::Keys { keys: k }, None, Duration::from_secs(30)).await, "admin_keys", "")
}
