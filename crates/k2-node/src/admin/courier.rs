//! The unprivileged node as courier (prd-k2-node-admin-v1 §5): it passes
//! command bytes to the root executor and results back. It can't approve,
//! widen or forge anything. Read verbs and job housekeeping it answers
//! itself, after checking the same controller signature against its own
//! pin.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use k2_node_proto::admin::{self, code, AdminKeys, AdminRefusal, AdminResult, Level, Runs, SignedCommand, UpdatePhase};
use k2_node_proto::frames::{Frame, UpdateChunk, UpdateOffer, UpdateState};
use tokio::sync::mpsc;
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
        Ok(LocalResponse::Job(_)) => AdminResult::refused(cmd_id, digest, &AdminRefusal::new(code::FAILED, "executor answered a job op")),
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
        "health" => {
            super::exec::only_keys(args, &["refresh_updates"])?;
            let refresh = args.get("refresh_updates").and_then(|v| v.as_bool()).unwrap_or(false);
            serde_json::to_value(super::health::collect(node, refresh).await).map_err(|e| AdminRefusal::new(code::FAILED, e.to_string()))
        }
        "ps" => {
            super::exec::no_args(args)?;
            let n = node.clone();
            tokio::task::spawn_blocking(move || super::housekeeping::ps(&n)).await.map_err(|e| AdminRefusal::new(code::FAILED, e.to_string()))
        }
        "jobs.kill" => super::housekeeping::kill(node, args),
        "gc" => super::housekeeping::gc(node, args).await,
        "policy.show" => {
            super::exec::no_args(args)?;
            Ok(super::policy::show(node.layout(), &node.facts, &node.session_narrow()))
        }
        _ => Err(AdminRefusal::new(code::VERB_NOT_ALLOWED, "verb is not built on this node yet")),
    }
}

/// Offered builds being received, by transfer id. In memory only: the
/// bytes go to the root executor over the local socket, never via a
/// node-writable spool path (§12.2).
#[derive(Default)]
pub struct IncomingUpdates(Mutex<HashMap<String, Incoming>>);

pub struct Incoming {
    offer: UpdateOffer,
    buf: Vec<u8>,
    next_seq: u32,
}

fn update_state(node: &Node, o: &UpdateOffer, phase: UpdatePhase, code_s: Option<&str>, message: Option<String>) -> Frame {
    Frame::UpdateState(UpdateState {
        transfer_id: Some(o.transfer_id.clone()),
        cmd_id: None,
        phase,
        version: o.version.clone(),
        from: Some(crate::PRODUCT_VERSION.into()),
        code: code_s.map(str::to_string),
        message,
        at: node.now(),
    })
}

/// `update_offer`: start receiving a build for this machine.
pub fn update_offer(node: &Arc<Node>, tx: &mpsc::Sender<Frame>, o: UpdateOffer) {
    let refuse = |c: &str, m: &str| {
        let _ = tx.try_send(update_state(node, &o, UpdatePhase::Refused, Some(c), Some(m.into())));
    };
    if o.size == 0 || o.size > super::server::MAX_UPDATE_BYTES {
        return refuse(code::TOO_LARGE, "build size out of range");
    }
    if o.os != crate::sysinfo::os_label() || o.arch != std::env::consts::ARCH {
        return refuse(code::BAD_BUILD, "build is for another os/arch");
    }
    let mut m = node.admin_updates.0.lock().unwrap();
    m.clear(); // one transfer at a time; a new offer replaces a stalled one
    m.insert(o.transfer_id.clone(), Incoming { buf: Vec::with_capacity(o.size as usize), next_seq: 0, offer: o });
}

/// `update_chunk`: append; on the last one hand the build to the executor.
pub fn update_chunk(node: &Arc<Node>, tx: &mpsc::Sender<Frame>, c: UpdateChunk) {
    let mut m = node.admin_updates.0.lock().unwrap();
    let Some(inc) = m.get_mut(&c.transfer_id) else { return };
    let fail = |inc: &Incoming, code_s: &str, msg: &str| update_state(node, &inc.offer, UpdatePhase::Failed, Some(code_s), Some(msg.into()));
    let data = match k2_node_proto::crypto::unb64(&c.data) {
        Ok(d) if c.seq == inc.next_seq && d.len() <= k2_node_proto::frames::TRANSFER_CHUNK => d,
        _ => {
            let f = fail(inc, code::MALFORMED, "chunk out of order or unreadable");
            m.remove(&c.transfer_id);
            let _ = tx.try_send(f);
            return;
        }
    };
    inc.buf.extend_from_slice(&data);
    inc.next_seq += 1;
    if inc.buf.len() as u64 > inc.offer.size {
        let f = fail(inc, code::TOO_LARGE, "more bytes than offered");
        m.remove(&c.transfer_id);
        let _ = tx.try_send(f);
        return;
    }
    if !c.last {
        return;
    }
    let Some(inc) = m.remove(&c.transfer_id) else { return };
    drop(m);
    let sha = k2_node_proto::crypto::sha256_hex(&inc.buf);
    if inc.buf.len() as u64 != inc.offer.size || !k2_node_proto::crypto::ct_eq(sha.as_bytes(), inc.offer.sha256.as_bytes()) {
        let _ = tx.try_send(fail(&inc, code::BAD_BUILD, "received bytes don't match the offer"));
        return;
    }
    let _ = tx.try_send(update_state(node, &inc.offer, UpdatePhase::Staged, None, None));
    let (n, t) = (node.clone(), tx.clone());
    tokio::spawn(async move {
        let req = LocalRequest::Update { cmd: inc.offer.cmd.clone(), sig: inc.offer.sig.clone(), size: inc.buf.len() as u64 };
        let r = as_result(ask(&n, req, Some(inc.buf), Duration::from_secs(180)).await, "", "");
        let f = if r.state == "done" {
            // The executor restarts this node next; this may not get out.
            update_state(&n, &inc.offer, UpdatePhase::Swapped, None, None)
        } else {
            let phase = if r.state == "failed" { UpdatePhase::Failed } else { UpdatePhase::Refused };
            update_state(&n, &inc.offer, phase, r.code.as_deref(), r.message.clone())
        };
        let _ = t.send(f).await;
    });
}

/// After an attach: report how the last self-update ended (healthy or
/// rolled back), from the executor's record. The controller dedups.
pub async fn report_last_update(node: Arc<Node>, tx: mpsc::Sender<Frame>) {
    let Ok(LocalResponse::Status(s)) = ask(&node, LocalRequest::Status, None, Duration::from_secs(5)).await else { return };
    let Some(v) = s.last_update else { return };
    let phase = match v.get("phase").and_then(|p| p.as_str()) {
        Some("healthy") => UpdatePhase::Healthy,
        Some("rolled_back") => UpdatePhase::RolledBack,
        _ => return,
    };
    let s_of = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let _ = tx
        .send(Frame::UpdateState(UpdateState {
            transfer_id: None,
            cmd_id: s_of("cmd_id"),
            phase,
            version: s_of("to").unwrap_or_default(),
            from: s_of("from"),
            code: None,
            message: s_of("reason"),
            at: v.get("at").and_then(|x| x.as_i64()).unwrap_or(0),
        }))
        .await;
}

/// `admin_keys` → the executor.
pub async fn keys(node: Arc<Node>, k: AdminKeys) -> AdminResult {
    as_result(ask(&node, LocalRequest::Keys { keys: k }, None, Duration::from_secs(30)).await, "admin_keys", "")
}
