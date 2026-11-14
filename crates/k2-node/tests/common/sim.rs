//! The server's half of an iPhone session, for tests: the stream
//! handshake, sealed requests, and the attach socket's sim states.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use k2_node_proto::frames::Frame;
use k2_node_proto::sim::ctl::{FromNode, Reply, ReplyBody, Request};
use k2_node_proto::sim::stream::{ControllerHandshake, Opener, Sealer, Side, StreamHandshake};
use k2_node_proto::sim::SessionState;
use tokio_tungstenite::tungstenite::Message;

use super::{Conn, Fake, SimWs, TestNode};

pub fn sid(n: u8) -> String {
    format!("{n:0>32x}")
}

/// The next sim state of `session` on the attach socket.
pub async fn state(c: &mut Conn, session: &str) -> (SessionState, Option<String>) {
    let seen = c.until(|f| matches!(f, Frame::SimState(s) if s.session == session)).await;
    match seen.last() {
        Some(Frame::SimState(s)) => (s.state, s.code.clone()),
        _ => unreachable!(),
    }
}

/// Take the node's next dialed stream and answer its handshake as the server.
pub async fn accept(fake: &mut Fake, node: &TestNode) -> (SimWs, Sealer, Opener) {
    let mut ws = tokio::time::timeout(Duration::from_secs(30), fake.sim_streams.recv()).await.expect("the node dialed no stream").unwrap();
    let Some(Ok(Message::Text(t))) = ws.next().await else { panic!("no hello") };
    let StreamHandshake::Hello(hello) = serde_json::from_str(&t).unwrap() else { panic!("not a hello") };
    let (pending, ch) = ControllerHandshake::answer(&fake.key, node.node.key.spki_der(), &hello).unwrap();
    ws.send(Message::Text(serde_json::to_string(&StreamHandshake::Challenge(ch)).unwrap())).await.unwrap();
    let Some(Ok(Message::Text(t))) = ws.next().await else { panic!("no proof") };
    let StreamHandshake::Proof(p) = serde_json::from_str(&t).unwrap() else { panic!("not a proof") };
    let (s, o) = pending.finish(&p).unwrap().split(Side::Controller);
    (ws, s, o)
}

/// Seal and send one message; returns what went on the wire.
pub async fn send(ws: &mut SimWs, s: &mut Sealer, channel: u8, plain: &[u8]) -> Vec<u8> {
    let msg = s.seal(channel, plain).unwrap();
    ws.send(Message::Binary(msg.clone())).await.unwrap();
    msg
}

/// The next opened message: (channel, plain).
pub async fn recv(ws: &mut SimWs, o: &mut Opener, within: Duration) -> (u8, Vec<u8>) {
    loop {
        let m = tokio::time::timeout(within, ws.next()).await.expect("nothing from the node").expect("stream ended").expect("read");
        if let Message::Binary(b) = m {
            return o.open(&b).unwrap();
        }
    }
}

/// A control request and its reply (other channels' messages are skipped).
pub async fn call(ws: &mut SimWs, s: &mut Sealer, o: &mut Opener, req: &Request, within: Duration) -> (ReplyBody, Vec<u8>) {
    let want = req.id();
    let msg = send(ws, s, 0, &serde_json::to_vec(req).unwrap()).await;
    loop {
        let (ch, plain) = recv(ws, o, within).await;
        if ch != 0 {
            continue;
        }
        if let Ok(FromNode::Reply(Reply { id, body })) = serde_json::from_slice(&plain) {
            if Some(id) == want {
                return (body, msg);
            }
        }
    }
}

pub fn simctl(set: &str, rest: &[&str]) -> Vec<String> {
    let mut v = vec!["simctl".to_string(), "--set".into(), set.into()];
    v.extend(rest.iter().map(|s| s.to_string()));
    v
}
