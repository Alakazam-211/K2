//! A session's stream (§3.4, IR-1, IR-2): k2-node dials
//! `wss://<server>/cli/compute/sim-stream` through the same routes as its
//! attach socket, proves its key in a handshake bound to this connection,
//! then relays: each sealed message from the server is opened and handed
//! to simhost as plain `channel ‖ payload`; each of simhost's is sealed and
//! sent. The relay sees sizes only, and a message from another connection
//! or the other direction never opens.
//!
//! While the lease lives, a dropped stream is redialed (1 s, then up to
//! 10 s apart); the lease table ends it after the grace (`lease_lost`).
//! simhost closing the session's socket ends every session
//! (`simhost_exited`): launchd restarts it, and its start sweep shuts the
//! devices down.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use k2_node_proto::sim::stream::{NodeHandshake, Opener, Sealer, Side, StreamHandshake, MAX_MESSAGE};
use k2_node_proto::sim::STREAM_PATH;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;

use crate::node::{Node, Tenant};
use crate::session::Ws;

/// How a relay ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// The server or the network: redial within the grace.
    Stream(String),
    /// simhost closed the session's socket.
    Simhost(String),
}

fn ws_config() -> WebSocketConfig {
    let mut c = WebSocketConfig::default();
    c.max_message_size = Some(MAX_MESSAGE + 4096);
    c.max_frame_size = Some(MAX_MESSAGE + 4096);
    c
}

async fn connect(base: &str) -> Result<Ws, String> {
    crate::session::init_tls();
    let url = crate::url::ws_url(base, STREAM_PATH)?;
    let fut = tokio_tungstenite::connect_async_with_config(url.as_str(), Some(ws_config()), false);
    match tokio::time::timeout(Duration::from_secs(5), fut).await {
        Err(_) => Err(format!("{url}: connect timed out")),
        Ok(Err(e)) => Err(format!("{url}: {e}")),
        Ok(Ok((ws, _))) => Ok(ws),
    }
}

/// The handshake on an open socket: this node's sealer and opener.
pub async fn handshake<S>(ws: &mut S, node: &Node, controller_spki: &[u8], session: &str) -> Result<(Sealer, Opener), String>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let (hs, hello) = NodeHandshake::start(&node.key, session)?;
    let text = serde_json::to_string(&StreamHandshake::Hello(hello)).map_err(|e| e.to_string())?;
    crate::session::send_text(ws, text).await?;
    let answer = tokio::time::timeout(Duration::from_secs(k2_node_proto::frames::HANDSHAKE_SECS), crate::session::recv_text(ws)).await.map_err(|_| "the server didn't answer the sim stream hello".to_string())??;
    let challenge = match serde_json::from_str::<StreamHandshake>(&answer).map_err(|e| format!("sim stream: {e}"))? {
        StreamHandshake::Challenge(c) => c,
        StreamHandshake::Refused(r) => return Err(format!("{}: {}", r.code, r.message)),
        _ => return Err("sim stream: expected a challenge".into()),
    };
    let (proof, keys) = hs.finish(&node.key, controller_spki, &challenge)?;
    let text = serde_json::to_string(&StreamHandshake::Proof(proof)).map_err(|e| e.to_string())?;
    crate::session::send_text(ws, text).await?;
    Ok(keys.split(Side::Node))
}

/// Relay between the server's socket and simhost's until either ends.
pub async fn relay<S>(ws: S, local: std::os::unix::net::UnixStream, mut sealer: Sealer, mut opener: Opener, ping_every: Duration) -> Ended
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin + Send + 'static,
{
    if local.set_nonblocking(true).is_err() {
        return Ended::Simhost("the Simulator service's socket".into());
    }
    let local = match tokio::net::UnixStream::from_std(local) {
        Ok(l) => l,
        Err(e) => return Ended::Simhost(format!("the Simulator service's socket: {e}")),
    };
    let (mut lr, mut lw) = local.into_split();
    let (to_ws, mut from_local) = tokio::sync::mpsc::channel::<(u8, Vec<u8>)>(64);
    let reader = tokio::spawn(async move {
        loop {
            let mut len = [0u8; 4];
            if lr.read_exact(&mut len).await.is_err() {
                return;
            }
            let n = u32::from_be_bytes(len) as usize;
            if n == 0 || n > super::local::MAX_MESSAGE {
                return;
            }
            let mut body = vec![0u8; n];
            if lr.read_exact(&mut body).await.is_err() {
                return;
            }
            let ch = body.remove(0);
            if to_ws.send((ch, body)).await.is_err() {
                return;
            }
        }
    });
    let (mut sink, mut stream) = ws.split();
    let mut ping = tokio::time::interval(ping_every);
    ping.tick().await;
    let end = loop {
        tokio::select! {
            m = stream.next() => match m {
                Some(Ok(Message::Binary(b))) => match opener.open(&b) {
                    Ok((ch, plain)) => {
                        if lw.write_all(&super::local::encode_frame(ch, &plain)).await.is_err() {
                            break Ended::Simhost("the Simulator service closed the session".into());
                        }
                    }
                    Err(e) => break Ended::Stream(e.to_string()),
                },
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Ok(Message::Text(_))) => break Ended::Stream("a text message after the handshake".into()),
                Some(Ok(_)) | None => break Ended::Stream("the server closed the stream".into()),
                Some(Err(e)) => break Ended::Stream(format!("read: {e}")),
            },
            f = from_local.recv() => match f {
                Some((ch, payload)) => match sealer.seal(ch, &payload) {
                    Ok(msg) => {
                        if sink.send(Message::Binary(msg)).await.is_err() {
                            break Ended::Stream("send failed".into());
                        }
                    }
                    Err(e) => break Ended::Stream(e.to_string()),
                },
                None => break Ended::Simhost("the Simulator service closed the session".into()),
            },
            _ = ping.tick() => {
                if sink.send(Message::Ping(Vec::new())).await.is_err() {
                    break Ended::Stream("ping failed".into());
                }
            }
        }
    };
    reader.abort();
    let _ = sink.close().await;
    end
}

/// Dial, relay and redial for one session until its lease ends.
pub async fn run(node: Arc<Node>, t: Arc<Tenant>, session: String, workspace_key: String) {
    let mut wait = Duration::from_secs(1);
    loop {
        if node.sim.leases.lock().unwrap().get(&session).is_none() {
            return;
        }
        match once(&node, &t, &session, &workspace_key).await {
            Ok(Ended::Simhost(why)) => {
                crate::nlog!("sim session {}…: {why}", &session[..8]);
                let e = node.sim.leases.lock().unwrap().simhost_exited();
                node.sim_apply(e);
                return;
            }
            Ok(Ended::Stream(why)) => {
                crate::nlog!("sim session {}…: stream down ({why}); redialing within the grace", &session[..8]);
                wait = Duration::from_secs(1);
            }
            Err(why) => crate::nlog!("sim session {}…: {why}", &session[..8]),
        }
        let e = node.sim.leases.lock().unwrap().stream_down(&session, node.now().max(0) as u64);
        node.sim_apply(e);
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(Duration::from_secs(10));
    }
}

/// One dial: every route in order until one takes the handshake.
async fn once(node: &Arc<Node>, t: &Arc<Tenant>, session: &str, workspace_key: &str) -> Result<Ended, String> {
    let spki = t.pin.lock().unwrap().as_ref().ok_or("this server isn't paired")?.controller_spki()?;
    let mut last = "no route to this server".to_string();
    for route in t.routes() {
        let mut ws = match connect(&route).await {
            Ok(ws) => ws,
            Err(e) => {
                last = e;
                continue;
            }
        };
        let (sealer, opener) = match handshake(&mut ws, node, &spki, session).await {
            Ok(x) => x,
            Err(e) => {
                last = e;
                continue;
            }
        };
        let socket = node.sim.opts.socket.clone();
        let (slot, key, sid) = (t.slot, workspace_key.to_string(), session.to_string());
        let local = match tokio::task::spawn_blocking(move || super::local::open_session(&socket, slot, &key, &sid)).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return Ok(Ended::Simhost(e)),
            Err(e) => return Ok(Ended::Simhost(e.to_string())),
        };
        let e = node.sim.leases.lock().unwrap().stream_up(session);
        node.sim_apply(e);
        return Ok(relay(ws, local, sealer, opener, node.opts.ping_every).await);
    }
    Err(last)
}
