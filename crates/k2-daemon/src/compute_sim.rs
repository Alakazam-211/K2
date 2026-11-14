//! iPhone on a Mac compute node (0.46.7, prd-ios-on-mac-node-0467-v1):
//! the daemon's half of `k2_core::sim::ios_node`.
//!
//! - [`Directory`]: the compute registry as the `ios-node` lane sees it
//!   (offers, grants, the attach socket, node keys).
//! - [`serve_sim_stream`]: `GET /cli/compute/sim-stream`, the one socket
//!   per sim session that k2-node dials. **Public route, fails closed**:
//!   the node proves its key in a handshake bound to a session this server
//!   opened on exactly that node; every message after it is AES-GCM with
//!   keys for this connection only (IR-1, IR-2). Nothing here logs a
//!   payload.
//! - [`on_sim_state`], [`on_offer`], [`sync_frame`]: the attach socket's
//!   sim frames and the reconcile after every attach (IR-7).

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use k2_core::compute::proto::crypto::{self, SigningKey};
use k2_core::compute::proto::frames::{self, Control, Frame, Refused};
use k2_core::compute::proto::sim::stream::{StreamHandshake, MAX_MESSAGE};
use k2_core::compute::proto::sim::{self as psim, SimFacts, SimStateFrame, SimSync};
use k2_core::compute::{self as compute, store};
use k2_core::log_debug;
use k2_core::sim::ios_node::{self, NodeDirectory, NodeInfo};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

pub const SIM_STREAM_PATH: &str = psim::STREAM_PATH;

/// The compute registry, for the `ios-node` lane.
pub struct Directory;

/// Why a node takes nothing new, in words; `None` = free.
pub fn hold_of(row: &store::NodeRow, offer: Option<&frames::Offer>) -> Option<String> {
    if let Some(p) = &row.controller_pause {
        return Some(format!("{p} on this server"));
    }
    let o = offer?;
    match o.control {
        Control::Paused => return Some("paused by its owner".into()),
        Control::Draining => return Some("draining".into()),
        Control::Stopped => return Some("stopped".into()),
        Control::Active => {}
    }
    o.refusal.as_ref().map(|r| {
        if r.code == k2_core::compute::proto::refusal::FOREIGN_LOCK {
            format!("holding for another team's job ({})", r.message)
        } else {
            r.message.clone()
        }
    })
}

fn info(row: &store::NodeRow, pinned: &compute::pinned_only::Pinned) -> NodeInfo {
    let (online, _, live) = crate::compute_ws::live_view(&row.id);
    let offer = live.or_else(|| row.last_offer.clone());
    let os = offer.as_ref().and_then(|o| o.labels.get("os").cloned()).or_else(|| row.os.clone());
    NodeInfo {
        id: row.id.clone(),
        name: row.name.clone(),
        online,
        protocol: offer.as_ref().map(|o| o.protocol).or(row.protocol.map(|p| p as u32)).unwrap_or(0),
        features: offer.as_ref().map(|o| o.features.clone()).unwrap_or_default(),
        mac: os.as_deref() == Some("macos"),
        sim: offer.as_ref().and_then(|o| o.sim.clone()),
        hold: hold_of(row, offer.as_ref()),
        pinned_only: pinned.holds(&row.id),
    }
}

impl NodeDirectory for Directory {
    fn nodes(&self) -> Vec<NodeInfo> {
        let (rows, pinned) = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            (store::list_nodes(&conn, false), compute::pinned_only::all(&conn))
        };
        rows.iter().filter(|r| r.state == "active").map(|r| info(r, &pinned)).collect()
    }

    fn granted(&self, node_id: &str, workspace_id: &str) -> bool {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::grant(&conn, node_id, workspace_id).is_some_and(|g| g.expires_at.is_none_or(|t| t > compute::now()))
    }

    fn allowed(&self, node_id: &str, workspace_id: &str) -> bool {
        let db = k2_core::db::shared();
        let conn = db.lock();
        compute::allow::allows(&conn, workspace_id, node_id)
    }

    fn send(&self, node_id: &str, frame: Frame) -> bool {
        crate::compute_ws::send_frame(node_id, frame)
    }

    fn node_spki(&self, node_id: &str) -> Option<Vec<u8>> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let row = store::node_by_id(&conn, node_id)?;
        crypto::pem_decode("PUBLIC KEY", &row.public_key_pem).ok()
    }

    fn node_by_fp(&self, fp: &str) -> Option<String> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::live_node_by_fp(&conn, fp).filter(|r| r.state == "active").map(|r| r.id)
    }

    fn controller_key(&self) -> Result<SigningKey, String> {
        compute::controller_key()
    }
}

/// At daemon start, before the sim lanes register (unix only, IR-12).
pub fn install() {
    ios_node::install(Arc::new(Directory));
}

/// The lane's view changed (an offer, a session ended): re-probe it so the
/// Mobile tab and `k2 sim status` see it.
fn reprobe() {
    k2_core::sim::manager::manager().refresh_probe(k2_core::sim::LaneKind::IosNode);
}

pub fn on_sim_state(node_id: &str, f: &SimStateFrame) {
    if let Some(l) = ios_node::lane() {
        l.on_state(node_id, f);
    }
}

pub fn on_offer(node_id: &str, sim: Option<&SimFacts>) {
    if let Some(l) = ios_node::lane() {
        l.on_offer(node_id, sim);
        reprobe();
    }
}

/// `sim_sync` for a node right after its attach (protocol ≥ 7 only; an
/// older node would drop the session on a frame it can't parse).
pub fn sync_frame(node_id: &str, node_protocol: u32) -> Option<Frame> {
    if node_protocol < psim::PROTOCOL_SIM {
        return None;
    }
    let held = ios_node::lane().map(|l| l.held_sessions(node_id)).unwrap_or_default();
    Some(Frame::SimSync(SimSync { sessions: held }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An older node never sees a sim frame: it would drop the session on
    /// a frame it can't parse.
    #[test]
    fn sim_sync_goes_only_to_protocol_7_nodes() {
        assert_eq!(sync_frame("n1", psim::PROTOCOL_SIM - 1), None);
        assert!(matches!(sync_frame("n1", psim::PROTOCOL_SIM), Some(Frame::SimSync(_))));
    }
}

// ── the stream socket ────────────────────────────────────────────────

type Ws<'a> = tokio_tungstenite::WebSocketStream<&'a mut TcpStream>;

async fn send_text(ws: &mut Ws<'_>, f: &StreamHandshake) -> bool {
    match serde_json::to_string(f) {
        Ok(t) => ws.send(Message::Text(t)).await.is_ok(),
        Err(_) => false,
    }
}

async fn refuse(ws: &mut Ws<'_>, code: &str, message: &str) {
    let _ = send_text(ws, &StreamHandshake::Refused(Refused { code: code.into(), message: message.into() })).await;
    let _ = ws.close(None).await;
}

async fn recv_text(ws: &mut Ws<'_>, deadline: tokio::time::Instant) -> Option<StreamHandshake> {
    loop {
        let msg = tokio::time::timeout_at(deadline, ws.next()).await.ok()??.ok()?;
        match msg {
            Message::Text(t) if t.len() <= frames::MAX_FRAME_BYTES => return serde_json::from_str(&t).ok(),
            Message::Ping(_) | Message::Pong(_) => continue,
            _ => return None,
        }
    }
}

/// `GET /cli/compute/sim-stream`. Caller already checked `compute::enabled()`.
pub async fn serve_sim_stream(stream: &mut TcpStream) {
    let Some(lane) = ios_node::lane() else { return };
    let mut ws = match tokio_tungstenite::accept_async(&mut *stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[compute] sim stream handshake failed: {e}");
            return;
        }
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(frames::HANDSHAKE_SECS);
    let Some(StreamHandshake::Hello(hello)) = recv_text(&mut ws, deadline).await else {
        return refuse(&mut ws, "protocol_error", "expected hello").await;
    };
    let l2 = lane.clone();
    let h2 = hello.clone();
    let answered = tokio::task::spawn_blocking(move || l2.stream_hello(&h2)).await;
    let (pending, challenge) = match answered {
        Ok(Ok(x)) => x,
        // One word for every refusal: nothing here tells a stranger which
        // sessions exist.
        _ => return refuse(&mut ws, "sim_stream_refused", "no such session for this node").await,
    };
    if !send_text(&mut ws, &StreamHandshake::Challenge(challenge)).await {
        return;
    }
    let Some(StreamHandshake::Proof(proof)) = recv_text(&mut ws, deadline).await else {
        return refuse(&mut ws, "protocol_error", "expected proof").await;
    };
    let session = pending.session_id().to_string();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let raw: ios_node::link::RawTx = Box::new(move |b| out_tx.send(b).is_ok());
    let conn = match pending.finish(&proof, raw) {
        Ok(c) => c,
        Err(_) => return refuse(&mut ws, "bad_proof", "handshake signature didn't verify").await,
    };
    let conn_id = conn.conn_id.clone();
    let (mut sink, mut source) = ws.split();
    // Messages are opened in order on one blocking worker.
    let (work_tx, work_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let worker_conn = conn.clone();
    let worker = tokio::task::spawn_blocking(move || {
        while let Ok(m) = work_rx.recv() {
            if worker_conn.deliver(&m).is_err() {
                break;
            }
        }
    });
    let mut ping = tokio::time::interval(Duration::from_secs(frames::PING_SECS));
    loop {
        tokio::select! {
            out = out_rx.recv() => match out {
                Some(b) => {
                    if sink.send(Message::Binary(b)).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
            msg = source.next() => match msg {
                Some(Ok(Message::Binary(b))) if b.len() <= MAX_MESSAGE => {
                    if work_tx.send(b).is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                _ => break,
            },
            _ = ping.tick() => {
                if sink.send(Message::Ping(Vec::new())).await.is_err() {
                    break;
                }
                if conn.is_closed() {
                    break;
                }
            }
        }
    }
    drop(work_tx);
    let _ = worker.await;
    conn.close("the stream socket closed");
    let l3 = lane.clone();
    let _ = tokio::task::spawn_blocking(move || l3.stream_closed(&session, &conn_id)).await;
    let _ = sink.close().await;
}
