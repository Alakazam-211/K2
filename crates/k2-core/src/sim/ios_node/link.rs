//! The server's end of one sim stream connection: AES-GCM both ways
//! (`k2_node_proto::sim::stream`), channel 0 for control requests, one
//! channel per Simulator helper. Blocking; the daemon's WebSocket task
//! feeds [`StreamConn::deliver`] and drains the raw sender.
//!
//! Nothing here logs a payload: requests are named by op only, and errors
//! never carry arguments, typed text or file contents (§3.7).

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use k2_node_proto::crypto;
use k2_node_proto::sim::ctl::{self, DiskOp, FromNode, ReplyBody, Request};
use k2_node_proto::sim::stream::{Opener, Sealer, Side, StreamKeys};
use serde_json::Value as J;

use crate::sim::ios::helper::{read_message, FrameSlot, HelperIo, Inbox, Message, OLDEST_PROTOCOL};
use crate::sim::ios::simctl::{Output, SimError, SimResult};

/// Sends one sealed binary message on the socket; `false` = the socket is gone.
pub type RawTx = Box<dyn Fn(Vec<u8>) -> bool + Send + Sync>;

/// How long a control request may take beyond its own timeout.
const SLACK: Duration = Duration::from_secs(10);
const DISK_TIMEOUT: Duration = Duration::from_secs(30);
const HELPER_READY: Duration = Duration::from_secs(60);

pub struct StreamConn {
    pub conn_id: String,
    sealer: Mutex<Sealer>,
    opener: Mutex<Opener>,
    raw: RawTx,
    pending: Mutex<HashMap<u64, mpsc::Sender<ReplyBody>>>,
    next_id: AtomicU64,
    helpers: Mutex<HashMap<u8, Arc<Inbox>>>,
    next_channel: AtomicU8,
    closed: AtomicBool,
    why: Mutex<Option<String>>,
    /// The node this connection reaches, for messages ("rig1-mac").
    node_name: String,
}

impl std::fmt::Debug for StreamConn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StreamConn({} on {})", self.conn_id, self.node_name)
    }
}

impl StreamConn {
    pub fn new(keys: StreamKeys, raw: RawTx, node_name: &str) -> Arc<Self> {
        let conn_id = keys.conn_id.clone();
        let (sealer, opener) = keys.split(Side::Controller);
        Arc::new(Self {
            conn_id,
            sealer: Mutex::new(sealer),
            opener: Mutex::new(opener),
            raw,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            helpers: Mutex::new(HashMap::new()),
            next_channel: AtomicU8::new(ctl::FIRST_HELPER),
            closed: AtomicBool::new(false),
            why: Mutex::new(None),
            node_name: node_name.to_string(),
        })
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// End the connection: every waiting call and helper wakes with an error.
    pub fn close(&self, why: &str) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        *self.why.lock().unwrap() = Some(why.to_string());
        self.pending.lock().unwrap().clear();
        for (_, ib) in self.helpers.lock().unwrap().drain() {
            ib.set_gone(format!("the connection to {} closed", self.node_name));
            ib.closed();
        }
    }

    fn gone(&self) -> SimError {
        SimError::new("node_reconnecting", format!("reconnecting to {}; try again in a moment", self.node_name))
    }

    /// Seal and send one message. The sealer's lock keeps send order = seq order.
    fn send(&self, channel: u8, plain: &[u8]) -> SimResult<()> {
        if self.is_closed() {
            return Err(self.gone());
        }
        let mut s = self.sealer.lock().unwrap();
        let msg = s.seal(channel, plain).map_err(|e| SimError::new("tool", e.to_string()))?;
        if (self.raw)(msg) {
            Ok(())
        } else {
            drop(s);
            self.close("the socket closed");
            Err(self.gone())
        }
    }

    /// One message from the node. An error is final: the caller closes the socket.
    pub fn deliver(&self, msg: &[u8]) -> Result<(), String> {
        let (channel, plain) = {
            let mut o = self.opener.lock().unwrap();
            match o.open(msg) {
                Ok(x) => x,
                Err(e) => {
                    let why = e.to_string();
                    drop(o);
                    self.close(&why);
                    return Err(why);
                }
            }
        };
        if channel == ctl::CONTROL {
            let m: FromNode = serde_json::from_slice(&plain).map_err(|_| "a control message from the node doesn't parse".to_string())?;
            match m {
                FromNode::Reply(r) => {
                    if let Some(tx) = self.pending.lock().unwrap().remove(&r.id) {
                        let _ = tx.send(r.body);
                    }
                }
                FromNode::Event(ctl::Event::HelperGone { channel, reason }) => {
                    if let Some(ib) = self.helpers.lock().unwrap().remove(&channel) {
                        ib.set_gone(reason);
                        ib.closed();
                    }
                }
            }
            return Ok(());
        }
        let ib = self.helpers.lock().unwrap().get(&channel).cloned();
        let Some(ib) = ib else { return Ok(()) };
        match read_message(&mut Cursor::new(&plain)) {
            Ok(Some(m)) => {
                let frame = matches!(m, Message::Frame(_));
                ib.dispatch(m);
                if frame {
                    // One frame in flight: the node sends the next one after this.
                    let ack = serde_json::to_vec(&Request::Ack { channel }).map_err(|e| e.to_string())?;
                    let _ = self.send(ctl::CONTROL, &ack);
                }
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(_) => Err("a helper message from the node doesn't parse".into()),
        }
    }

    /// Send a control request and wait for its reply.
    fn request(&self, make: impl FnOnce(u64) -> Request, timeout: Duration) -> SimResult<ReplyBody> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = make(id);
        let op = req.op();
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let body = serde_json::to_vec(&req).map_err(|e| SimError::new("tool", e.to_string()))?;
        if let Err(e) = self.send(ctl::CONTROL, &body) {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        match rx.recv_timeout(timeout) {
            Ok(ReplyBody::Refused { message, .. }) => Err(SimError::new("node_refused", message)),
            Ok(r) => Ok(r),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.pending.lock().unwrap().remove(&id);
                Err(SimError::new("timeout", format!("{} didn't answer {op} within {}s", self.node_name, timeout.as_secs())))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(self.gone()),
        }
    }

    fn failed(code: &str, message: String) -> SimError {
        // Keep the node's teaching message; the code is only ever one of ours.
        let c = match code {
            "timeout" => "timeout",
            "no_device" => "no_device",
            _ => "node_failed",
        };
        SimError::new(c, message)
    }

    pub fn run(&self, program: &str, args: &[String], timeout: Duration) -> SimResult<Output> {
        let ms = timeout.as_millis().min(u64::MAX as u128) as u64;
        match self.request(|id| Request::Run { id, program: program.to_string(), args: args.to_vec(), timeout_ms: ms }, timeout + SLACK)? {
            ReplyBody::Run { status, stdout, stderr } => Ok(Output { status, stdout, stderr }),
            ReplyBody::Failed { code, message } => Err(Self::failed(&code, message)),
            _ => Err(SimError::new("tool", "unexpected reply to run")),
        }
    }

    pub fn disk(&self, op: DiskOp, path: &str) -> SimResult<ReplyBody> {
        match self.request(|id| Request::Disk { id, op, path: path.to_string() }, DISK_TIMEOUT)? {
            ReplyBody::Failed { code, message } => Err(Self::failed(&code, message)),
            r => Ok(r),
        }
    }

    /// Upload a packed `.app` (`pack::pack_app`); the node unpacks it under
    /// this server's uploads and answers the `.app`'s path there.
    pub fn upload(&self, bytes: &[u8]) -> SimResult<String> {
        let sha = crypto::sha256_hex(bytes);
        let total = bytes.len() as u64;
        let chunks: Vec<&[u8]> = if bytes.is_empty() { vec![&[][..]] } else { bytes.chunks(ctl::UPLOAD_CHUNK).collect() };
        let n = chunks.len();
        let mut offset = 0u64;
        for (i, c) in chunks.into_iter().enumerate() {
            let last = i + 1 == n;
            let data = crypto::b64(c);
            let r = self.request(|id| Request::Upload { id, sha256: sha.clone(), total, offset, data, last }, Duration::from_secs(if last { 300 } else { 60 }))?;
            offset += c.len() as u64;
            match r {
                ReplyBody::Path { path } if last => return Ok(path),
                ReplyBody::Ok if !last => {}
                ReplyBody::Failed { message, .. } => return Err(SimError::new("bad_app", message)),
                _ => return Err(SimError::new("tool", "unexpected reply to upload")),
            }
        }
        Err(SimError::new("tool", "upload ended without a path"))
    }

    /// Start the helper for `udid` on a fresh channel; returns once it said `ready`.
    pub fn start_helper(self: &Arc<Self>, udid: &str) -> SimResult<Arc<RemoteHelper>> {
        let channel = self.next_channel.fetch_add(1, Ordering::SeqCst);
        if channel == 0 {
            return Err(SimError::new("tool", "out of helper channels on this connection"));
        }
        // v1 keeps a node's helper on JPEG frames (protocol 3): sized to the
        // viewer at 8-15 fps, they fit the relay; H.264 over a node is later.
        let (inbox, ready) = Inbox::starting(Some(OLDEST_PROTOCOL));
        self.helpers.lock().unwrap().insert(channel, inbox.clone());
        match self.request(|id| Request::HelperStart { id, channel, udid: udid.to_string() }, HELPER_READY) {
            Ok(ReplyBody::Ok) => {}
            Ok(ReplyBody::Failed { code, message }) => {
                self.helpers.lock().unwrap().remove(&channel);
                return Err(Self::failed(&code, message));
            }
            Ok(_) => return Err(SimError::new("tool", "unexpected reply to helper_start")),
            Err(e) => {
                self.helpers.lock().unwrap().remove(&channel);
                return Err(e);
            }
        }
        match ready.recv_timeout(HELPER_READY) {
            Ok(Ok(_)) => Ok(Arc::new(RemoteHelper { conn: self.clone(), channel, inbox })),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(SimError::new("helper_timeout", format!("the Simulator helper on {} didn't start", self.node_name))),
        }
    }

    fn stop_helper(&self, channel: u8) {
        if self.helpers.lock().unwrap().remove(&channel).is_some() && !self.is_closed() {
            let _ = self.request(|id| Request::HelperStop { id, channel }, Duration::from_secs(5));
        }
    }
}

/// A helper on the node, reached over one channel of the stream.
pub struct RemoteHelper {
    conn: Arc<StreamConn>,
    channel: u8,
    inbox: Arc<Inbox>,
}

impl HelperIo for RemoteHelper {
    fn call(&self, op: &str, args: J, timeout: Duration) -> SimResult<J> {
        self.inbox.request(
            op,
            args,
            timeout,
            |line| self.conn.send(self.channel, line).map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e.message)),
            String::new,
        )
    }
    /// A touch's down and moves: one `nr` line, no reply (fix 3). The node
    /// forwards it like any other line on the helper's channel.
    fn notify(&self, op: &str, args: J) -> SimResult<()> {
        self.inbox.notify(op, args, |line| self.conn.send(self.channel, line).map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e.message)))
    }
    fn take_input_error(&self) -> Option<SimError> {
        self.inbox.take_input_error()
    }
    fn frame_slot(&self, slot: Option<Arc<FrameSlot>>) {
        self.inbox.set_frame_slot(slot);
    }
    fn alive(&self) -> bool {
        self.inbox.is_alive() && !self.conn.is_closed()
    }
    fn protocol(&self) -> u32 {
        self.inbox.protocol()
    }
}

impl Drop for RemoteHelper {
    fn drop(&mut self) {
        self.conn.stop_helper(self.channel);
    }
}
