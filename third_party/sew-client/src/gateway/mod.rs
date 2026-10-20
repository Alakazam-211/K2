//! The gateway client (S25 §10, C7): a sans-IO [`machine::Machine`] plus a thin async driver.
//!
//! **Pull-driven.** The library spawns nothing: all progress (connect, HELLO, IDENTIFY/RESUME,
//! heartbeats, reconnects, REAUTH) happens inside [`Gateway::next_event`]. A caller that stops
//! polling for longer than 1.5 × the heartbeat interval is closed by the server with 4009; the
//! next poll resumes. Callers that want a background task spawn one themselves and forward
//! events over a channel.
//!
//! **Cancel-safe.** Timers are deadlines stored in the gateway, outgoing frames are queued in it
//! and flushed with `poll_ready`/`start_send`, and a connect, ticket mint or refresh in flight is
//! a boxed future kept in the gateway: dropping a `next_event` future loses no frame and no state.

use std::collections::VecDeque;
use std::fmt;
use std::future::{Future, poll_fn};
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::Poll;
use std::time::Duration;

use futures_util::sink::Sink;
use futures_util::stream::{SplitSink, SplitStream, StreamExt as _};
use serde_json::Value;
use sew_proto::gateway::client::{ClientInfo, PresenceSetStatus, SubscribeRequest, TypingRequest};
use sew_proto::gateway::server::{Ready, ServerOp};
use sew_proto::gateway::{Event, Frame};
use sew_proto::id::Id;
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode as WsCloseCode;
use url::Url;

use crate::api::Client;
use crate::error::{ClientError, GatewayError};
use crate::retry::Backoff;

mod conn;
pub mod machine;

use conn::{SocketOpts, Ws, ZstdStream};
pub use machine::GatewayState;
use machine::{Action, Input, Machine, MachineConfig, TimerKind};

/// How often a gateway with a pass source checks whether the session needs a renew.
pub const RENEW_CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// What [`Gateway::next_event`] returns.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum GatewayEvent {
    /// A fresh IDENTIFY succeeded: the caller must catch up (S25 §13).
    Ready(Box<Ready>),
    /// RESUME worked; nothing was lost. `replayed` dispatches follow as `Dispatch`.
    Resumed {
        /// Dispatches the server replayed between RESUME and `resumed`.
        replayed: u32,
    },
    /// Every other dispatch, `Event::Unknown` included (C7: unknown `t` is not hidden).
    Dispatch {
        /// The session seq.
        s: u64,
        /// The event.
        event: Event,
    },
    /// The socket was lost; emitted once per socket with the next step and the wait.
    Disconnected {
        /// The close code, when there was one (4000 when the client reconnects on purpose).
        code: Option<u16>,
        /// The close reason, or what failed.
        reason: String,
        /// Resume or identify next.
        next: NextStep,
        /// The wait before the next connect.
        after: Duration,
    },
    /// Terminal: `next_event` returns `Err(Closed)` afterwards.
    Stopped(StopReason),
}

/// The handshake after the next connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NextStep {
    /// RESUME the held session.
    Resume,
    /// IDENTIFY a new session.
    Identify,
}

/// Why the gateway stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StopReason {
    /// 4011: the session was revoked or the account removed: drop local data.
    Revoked,
    /// 4004 with no way to get a fresh credential, or the re-login failed.
    AuthFailed,
    /// 4012: this client's protocol version is not served.
    InvalidVersion,
    /// A close that means a client bug (4005 without `resumed_elsewhere`).
    ClientBug(u16),
    /// 4005 `resumed_elsewhere`: another socket took this session over (do not auto-resume).
    ResumedElsewhere,
    /// `max_reconnect_attempts` failed attempts in a row.
    GaveUp,
    /// The caller closed the gateway.
    UserClosed,
}

/// How the gateway authenticates each connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GatewayAuthMode {
    /// The session token in IDENTIFY/RESUME (default).
    #[default]
    Token,
    /// Mint a single-use ticket over HTTP before each connect (S8 §10); connect to its URL.
    Ticket,
}

/// Gateway options. Build with `GatewayOptions::default()` and the setters.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct GatewayOptions {
    /// Token (default) or ticket.
    pub auth: GatewayAuthMode,
    /// Ask for `compress=zstd-stream` (default false).
    pub compress: bool,
    /// Sent in IDENTIFY (default `{name: "sew-client", version: <crate version>}`).
    pub client: ClientInfo,
    /// Subscriptions (re)sent after every READY.
    pub subscriptions: Vec<SubscribeRequest>,
    /// Presence (re)sent after every READY.
    pub presence: Option<PresenceSetStatus>,
    /// Reconnect backoff base (C7: 1 s).
    pub backoff_base: Duration,
    /// Reconnect backoff cap (C7: 60 s).
    pub backoff_cap: Duration,
    /// Stop with `GaveUp` after this many failed attempts in a row (`None` = forever).
    pub max_reconnect_attempts: Option<u32>,
    /// Overrides the gateway URL (default: from the base URL, or the ticket's).
    pub url: Option<Url>,
    /// Bind the socket to this local address (S26, contracts rev 35).
    pub local_addr: Option<SocketAddr>,
    /// `SO_RCVBUF` (S26).
    pub recv_buffer_bytes: Option<u32>,
    /// `SO_SNDBUF` (S26).
    pub send_buffer_bytes: Option<u32>,
    /// TCP + TLS + websocket handshake timeout (default 10 s).
    pub connect_timeout: Duration,
    /// Keep every frame in and out for [`Gateway::take_frames`] (`sewc gateway watch --raw`).
    pub record_frames: bool,
}

impl Default for GatewayOptions {
    fn default() -> Self {
        GatewayOptions {
            auth: GatewayAuthMode::Token,
            compress: false,
            client: ClientInfo {
                name: "sew-client".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            subscriptions: Vec::new(),
            presence: None,
            backoff_base: Backoff::GATEWAY.base,
            backoff_cap: Backoff::GATEWAY.cap,
            max_reconnect_attempts: None,
            url: None,
            local_addr: None,
            recv_buffer_bytes: None,
            send_buffer_bytes: None,
            connect_timeout: Duration::from_secs(10),
            record_frames: false,
        }
    }
}

impl GatewayOptions {
    /// Token or ticket mode.
    pub fn auth(mut self, mode: GatewayAuthMode) -> Self {
        self.auth = mode;
        self
    }

    /// `zstd-stream` on or off.
    pub fn compress(mut self, on: bool) -> Self {
        self.compress = on;
        self
    }

    /// The IDENTIFY client info.
    pub fn client_info(mut self, name: &str, version: &str) -> Self {
        self.client = ClientInfo {
            name: name.to_owned(),
            version: version.to_owned(),
        };
        self
    }

    /// Adds a subscription.
    pub fn subscribe(mut self, req: SubscribeRequest) -> Self {
        self.subscriptions.push(req);
        self
    }

    /// The presence to send after READY.
    pub fn presence(mut self, status: PresenceSetStatus) -> Self {
        self.presence = Some(status);
        self
    }

    /// The reconnect backoff.
    pub fn backoff(mut self, base: Duration, cap: Duration) -> Self {
        self.backoff_base = base;
        self.backoff_cap = cap;
        self
    }

    /// Give up after `n` failed attempts in a row.
    pub fn max_reconnect_attempts(mut self, n: u32) -> Self {
        self.max_reconnect_attempts = Some(n);
        self
    }

    /// Overrides the gateway URL.
    pub fn url(mut self, url: Url) -> Self {
        self.url = Some(url);
        self
    }

    /// Binds the socket to a local address.
    pub fn local_addr(mut self, addr: SocketAddr) -> Self {
        self.local_addr = Some(addr);
        self
    }

    /// Socket buffer sizes.
    pub fn buffers(mut self, recv: Option<u32>, send: Option<u32>) -> Self {
        self.recv_buffer_bytes = recv;
        self.send_buffer_bytes = send;
        self
    }

    /// The connect timeout.
    pub fn connect_timeout(mut self, d: Duration) -> Self {
        self.connect_timeout = d;
        self
    }

    /// Record frames for [`Gateway::take_frames`].
    pub fn record_frames(mut self, on: bool) -> Self {
        self.record_frames = on;
        self
    }
}

/// Which way a recorded frame went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameDir {
    /// Server to client.
    In,
    /// Client to server.
    Out,
}

impl FrameDir {
    /// `in` or `out`.
    pub fn as_str(self) -> &'static str {
        match self {
            FrameDir::In => "in",
            FrameDir::Out => "out",
        }
    }
}

/// A recorded frame (tokens and tickets in outgoing frames are redacted).
#[derive(Debug, Clone, PartialEq)]
pub struct FrameRecord {
    /// Direction.
    pub dir: FrameDir,
    /// The frame.
    pub frame: Frame,
}

/// Most frames kept for [`Gateway::take_frames`].
const FRAME_LOG_MAX: usize = 10_000;
/// How long a closing socket may take to send its close frame.
const CLOSE_FLUSH: Duration = Duration::from_secs(2);

enum PendingOut {
    Connected { ws: Box<Ws>, ticket: Option<String> },
    ConnectFailed { code: Option<u16>, reason: String },
    Refreshed(Result<(), ClientError>),
    Checked(Result<(), ClientError>),
}

type PendingFut = Pin<Box<dyn Future<Output = PendingOut> + Send>>;

/// A closing socket: its close frame is flushed until done or the deadline.
struct Closing {
    sink: SplitSink<Ws, Message>,
    out: VecDeque<Message>,
    deadline: Instant,
}

/// A gateway connection that reconnects by itself. See the module docs.
pub struct Gateway {
    client: Client,
    opts: GatewayOptions,
    machine: Machine,
    base_url: Url,
    epoch: Instant,
    sink: Option<SplitSink<Ws, Message>>,
    stream: Option<SplitStream<Ws>>,
    zstd: Option<ZstdStream>,
    out: VecDeque<Message>,
    closing: Option<Closing>,
    events: VecDeque<GatewayEvent>,
    timers: [Option<Instant>; 3],
    connect_at: Option<Instant>,
    pending: Option<PendingFut>,
    renew_check_at: Option<Instant>,
    token_rx: Option<watch::Receiver<u64>>,
    frames: VecDeque<FrameRecord>,
    started: bool,
    stopped: bool,
}

impl fmt::Debug for Gateway {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Gateway")
            .field("url", &self.base_url.as_str())
            .field("machine", &self.machine)
            .field("connected", &self.stream.is_some())
            .finish_non_exhaustive()
    }
}

fn timer_index(t: TimerKind) -> usize {
    match t {
        TimerKind::HelloTimeout => 0,
        TimerKind::Heartbeat => 1,
        TimerKind::IdentifyGate => 2,
    }
}

/// `ws(s)://<host>[/prefix]/api/v1/gateway?v=1&encoding=json[&compress=zstd-stream]`.
pub fn gateway_url(base: &str, compress: bool) -> Result<Url, ClientError> {
    let mut url =
        Url::parse(base).map_err(|e| ClientError::Config(format!("base URL {base:?}: {e}")))?;
    let scheme = match url.scheme() {
        "http" => "ws",
        "https" => "wss",
        s => return Err(ClientError::Config(format!("base URL scheme {s:?}"))),
    };
    url.set_scheme(scheme)
        .map_err(|()| ClientError::Config(format!("cannot use {scheme} for {base:?}")))?;
    let path = format!(
        "{}{}",
        url.path().trim_end_matches('/'),
        sew_proto::routes::GATEWAY
    );
    url.set_path(&path);
    url.set_query(Some("v=1&encoding=json"));
    if compress {
        url.query_pairs_mut().append_pair("compress", "zstd-stream");
    }
    Ok(url)
}

/// A ticket's URL with `v`/`encoding` kept (rev 22) and `compress` added when asked.
fn ticket_url(url: &str, compress: bool) -> Result<Url, String> {
    let mut u = Url::parse(url).map_err(|e| format!("ticket URL {url:?}: {e}"))?;
    if u.query().is_none() {
        u.set_query(Some("v=1&encoding=json"));
    }
    if compress && !u.query_pairs().any(|(k, _)| k == "compress") {
        u.query_pairs_mut().append_pair("compress", "zstd-stream");
    }
    Ok(u)
}

/// A frame with its `token`/`ticket` values cut, for the frame log.
fn redacted(frame: &Frame) -> Frame {
    let mut f = frame.clone();
    if let Value::Object(m) = &mut f.d {
        for k in ["token", "ticket"] {
            if let Some(v) = m.get_mut(k)
                && let Some(s) = v.as_str()
            {
                let prefix = s.split('_').next().unwrap_or("");
                *v = Value::String(format!("{prefix}_***"));
            }
        }
    }
    f
}

async fn sleep_until_opt(at: Option<Instant>) {
    match at {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending().await,
    }
}

type WsResult<T> = Result<T, tokio_tungstenite::tungstenite::Error>;

/// Flushes `out` into `sink` (cancel-safe: a message leaves `out` only once the sink took it).
async fn flush(sink: &mut SplitSink<Ws, Message>, out: &mut VecDeque<Message>) -> WsResult<()> {
    poll_fn(|cx| {
        while !out.is_empty() {
            match Pin::new(&mut *sink).poll_ready(cx) {
                Poll::Ready(Ok(())) => {}
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
            if let Some(m) = out.pop_front()
                && let Err(e) = Pin::new(&mut *sink).start_send(m)
            {
                return Poll::Ready(Err(e));
            }
        }
        Pin::new(&mut *sink).poll_flush(cx)
    })
    .await
}

async fn next_msg(stream: &mut Option<SplitStream<Ws>>) -> Option<WsResult<Message>> {
    match stream.as_mut() {
        Some(s) => s.next().await,
        None => std::future::pending().await,
    }
}

async fn flush_opt(
    sink: &mut Option<SplitSink<Ws, Message>>,
    out: &mut VecDeque<Message>,
) -> WsResult<()> {
    match sink.as_mut() {
        Some(s) => flush(s, out).await,
        None => std::future::pending().await,
    }
}

async fn drive_closing(c: &mut Option<Closing>) {
    match c.as_mut() {
        Some(cl) => {
            let deadline = cl.deadline;
            let _ = tokio::time::timeout_at(deadline, async {
                let _ = flush(&mut cl.sink, &mut cl.out).await;
                let _ = poll_fn(|cx| Pin::new(&mut cl.sink).poll_close(cx)).await;
            })
            .await;
        }
        None => std::future::pending().await,
    }
}

async fn run_pending(p: &mut Option<PendingFut>) -> PendingOut {
    match p.as_mut() {
        Some(f) => f.await,
        None => std::future::pending().await,
    }
}

enum Wake {
    Pending(PendingOut),
    Msg(Option<WsResult<Message>>),
    Flushed(WsResult<()>),
    ClosingDone,
    Timer,
    Connect,
    RenewCheck,
    Token,
}

impl Client {
    /// The gateway for this client's session (token mode needs a token; ticket mode mints a
    /// ticket before each connect). `Err(Unsupported)` when acting as an agent (agents have no
    /// gateway, C9). Nothing connects until the first [`Gateway::next_event`].
    pub fn gateway(&self, opts: GatewayOptions) -> Result<Gateway, ClientError> {
        self.refuse_act_as("agents have no gateway (C9)")?;
        let cell = self.session_cell()?;
        let token = cell.token();
        if token.is_none() {
            return Err(ClientError::Config(
                "the gateway needs a session token; log in first".to_owned(),
            ));
        }
        for s in &opts.subscriptions {
            s.check_limits()
                .map_err(|e| ClientError::invalid(&e.field, e.hint))?;
        }
        let base_url = match &opts.url {
            Some(u) => u.clone(),
            None => gateway_url(&self.base_url(), opts.compress)?,
        };
        let mut token_rx = cell.subscribe();
        let _ = token_rx.borrow_and_update();
        let cfg = MachineConfig {
            client: opts.client.clone(),
            can_refresh: self.has_pass_source(),
            backoff: Backoff {
                base: opts.backoff_base,
                cap: opts.backoff_cap,
            },
            max_reconnect_attempts: opts.max_reconnect_attempts,
        };
        let mut machine = Machine::new(cfg, token);
        let epoch = Instant::now();
        for s in &opts.subscriptions {
            machine.handle(Duration::ZERO, Input::Subscribe(s.clone()));
        }
        if let Some(p) = opts.presence {
            machine.handle(Duration::ZERO, Input::PresenceSet(p));
        }
        let renew_check_at = self.has_pass_source().then(|| epoch + RENEW_CHECK_INTERVAL);
        Ok(Gateway {
            client: self.clone(),
            opts,
            machine,
            base_url,
            epoch,
            sink: None,
            stream: None,
            zstd: None,
            out: VecDeque::new(),
            closing: None,
            events: VecDeque::new(),
            timers: [None; 3],
            connect_at: None,
            pending: None,
            renew_check_at,
            token_rx: Some(token_rx),
            frames: VecDeque::new(),
            started: false,
            stopped: false,
        })
    }
}

impl Gateway {
    /// Drives everything and returns the next event. After `Stopped`, returns `Err(Closed)`.
    ///
    /// Cancel-safe: dropping the future loses no frame and no state.
    pub async fn next_event(&mut self) -> Result<GatewayEvent, GatewayError> {
        loop {
            if let Some(ev) = self.events.pop_front() {
                return Ok(ev);
            }
            if self.stopped {
                return Err(GatewayError::Closed);
            }
            if !self.started {
                self.started = true;
                self.feed(Input::Start);
                continue;
            }
            let wake = self.wait().await;
            self.on_wake(wake);
        }
    }

    /// Stores a subscription for its scope (a new one replaces the old) and sends it now when
    /// live. Checks C7 limits locally first (100 typing, 500 presence, ≤ 5 ranges of ≤ 100):
    /// over a limit is `InvalidInput` and nothing is stored or sent.
    pub async fn subscribe(&mut self, req: SubscribeRequest) -> Result<(), GatewayError> {
        if self.stopped {
            return Err(GatewayError::Closed);
        }
        req.check_limits().map_err(|e| GatewayError::InvalidInput {
            field: e.field,
            hint: e.hint,
        })?;
        self.feed(Input::Subscribe(req));
        self.flush_now().await;
        Ok(())
    }

    /// TYPING in a channel: one per channel per 8 s (extra calls send nothing); dropped while
    /// disconnected.
    pub async fn typing(
        &mut self,
        channel_id: Id,
        thread_id: Option<Id>,
    ) -> Result<(), GatewayError> {
        if self.stopped {
            return Err(GatewayError::Closed);
        }
        self.feed(Input::Typing(TypingRequest {
            channel_id,
            thread_id,
        }));
        self.flush_now().await;
        Ok(())
    }

    /// Stores the presence status and sends it now when live, and after every READY.
    pub async fn presence_set(&mut self, status: PresenceSetStatus) -> Result<(), GatewayError> {
        if self.stopped {
            return Err(GatewayError::Closed);
        }
        self.feed(Input::PresenceSet(status));
        self.flush_now().await;
        Ok(())
    }

    /// Closes with 1000 (the session ends at once, gap S25-G8).
    pub async fn close(mut self) {
        self.feed(Input::UserClose);
        if let Some(mut c) = self.closing.take() {
            let _ = tokio::time::timeout_at(c.deadline, async {
                let _ = flush(&mut c.sink, &mut c.out).await;
                let _ = poll_fn(|cx| Pin::new(&mut c.sink).poll_close(cx)).await;
            })
            .await;
        }
    }

    /// Drops the TCP connection without a close frame (resume tests, `sewc gateway
    /// resume-test`). The session stays resumable; the next `next_event` reconnects after the
    /// backoff.
    pub fn cut(&mut self) {
        if self.stream.is_none() && self.sink.is_none() {
            return;
        }
        self.sink = None;
        self.stream = None;
        self.zstd = None;
        self.out.clear();
        self.feed(Input::Closed {
            code: None,
            reason: "connection cut by the client".to_owned(),
        });
    }

    /// The machine's phase.
    pub fn state(&self) -> GatewayState {
        self.machine.state()
    }

    /// The current session id.
    pub fn session_id(&self) -> Option<&str> {
        self.machine.session_id()
    }

    /// The last dispatch seq applied.
    pub fn last_seq(&self) -> Option<u64> {
        self.machine.last_seq()
    }

    /// The frames recorded since the last call (with `record_frames`).
    pub fn take_frames(&mut self) -> Vec<FrameRecord> {
        self.frames.drain(..).collect()
    }

    /// The URL identify and resume connect to (the ticket's in ticket mode).
    pub fn url(&self) -> &Url {
        &self.base_url
    }

    // ------------------------------------------------------------------ driver internals

    fn now(&self) -> Duration {
        self.epoch.elapsed()
    }

    fn record(&mut self, dir: FrameDir, frame: &Frame) {
        if !self.opts.record_frames {
            return;
        }
        if self.frames.len() >= FRAME_LOG_MAX {
            self.frames.pop_front();
        }
        let frame = match dir {
            FrameDir::Out => redacted(frame),
            FrameDir::In => frame.clone(),
        };
        self.frames.push_back(FrameRecord { dir, frame });
    }

    fn feed(&mut self, input: Input) {
        let now = self.now();
        let actions = self.machine.handle(now, input);
        for a in actions {
            self.perform(a);
        }
    }

    fn perform(&mut self, a: Action) {
        let now = Instant::now();
        match a {
            Action::Connect { after } => {
                self.connect_at = Some(now + after);
            }
            Action::Send(op) => {
                let frame = op.encode();
                self.record(FrameDir::Out, &frame);
                match serde_json::to_string(&frame) {
                    Ok(text) => self.out.push_back(Message::text(text)),
                    Err(e) => {
                        tracing::error!(error = %e, "sew-client gateway: frame does not encode")
                    }
                }
            }
            Action::Emit(ev) => {
                if matches!(ev, GatewayEvent::Stopped(_)) {
                    self.stopped = true;
                    self.connect_at = None;
                    self.pending = None;
                    self.timers = [None; 3];
                }
                self.events.push_back(ev);
            }
            Action::SetTimer(t, d) => self.timers[timer_index(t)] = Some(now + d),
            Action::CancelTimer(t) => self.timers[timer_index(t)] = None,
            Action::CloseSocket(code) => self.close_socket(code),
            Action::Refresh => {
                let client = self.client.clone();
                self.pending = Some(Box::pin(async move {
                    PendingOut::Refreshed(client.refresh_for_gateway().await)
                }));
            }
        }
    }

    /// Sends a close frame with `code` on the current socket (bounded) and forgets the socket.
    fn close_socket(&mut self, code: u16) {
        self.stream = None;
        self.zstd = None;
        let pending_out = std::mem::take(&mut self.out);
        if let Some(sink) = self.sink.take() {
            let mut out = pending_out;
            out.push_back(Message::Close(Some(CloseFrame {
                code: WsCloseCode::from(code),
                reason: "".into(),
            })));
            self.closing = Some(Closing {
                sink,
                out,
                deadline: Instant::now() + CLOSE_FLUSH,
            });
        }
    }

    fn start_connect(&mut self) {
        let client = self.client.clone();
        let mode = self.opts.auth;
        let compress = self.opts.compress;
        let url_override = self.opts.url.clone();
        let base = self.base_url.clone();
        let sock = SocketOpts {
            local_addr: self.opts.local_addr,
            recv_buffer_bytes: self.opts.recv_buffer_bytes,
            send_buffer_bytes: self.opts.send_buffer_bytes,
            connect_timeout: self.opts.connect_timeout,
        };
        self.pending = Some(Box::pin(async move {
            let (url, ticket) = match mode {
                GatewayAuthMode::Token => (base, None),
                GatewayAuthMode::Ticket => match client.gateway_ticket().await {
                    Ok(t) => {
                        let url = match (&url_override, ticket_url(&t.url, compress)) {
                            (Some(u), _) => u.clone(),
                            (None, Ok(u)) => u,
                            (None, Err(e)) => {
                                return PendingOut::ConnectFailed {
                                    code: None,
                                    reason: e,
                                };
                            }
                        };
                        (url, Some(t.ticket))
                    }
                    Err(e) if e.is_auth() => {
                        let reason = if e.code() == Some(sew_proto::error::Code::SessionExpired) {
                            sew_proto::gateway::CLOSE_REASON_SESSION_EXPIRED
                        } else {
                            "auth_failed"
                        };
                        return PendingOut::ConnectFailed {
                            code: Some(4004),
                            reason: reason.to_owned(),
                        };
                    }
                    Err(e) => {
                        return PendingOut::ConnectFailed {
                            code: None,
                            reason: format!("gateway ticket: {e}"),
                        };
                    }
                },
            };
            match conn::connect(&url, &sock).await {
                Ok(ws) => PendingOut::Connected {
                    ws: Box::new(ws),
                    ticket,
                },
                Err(reason) => PendingOut::ConnectFailed { code: None, reason },
            }
        }));
    }

    async fn wait(&mut self) -> Wake {
        let timer_at = self.timers.iter().flatten().min().copied();
        let connect_at = if self.pending.is_none() {
            self.connect_at
        } else {
            None
        };
        let renew_at = if self.pending.is_none() && self.connect_at.is_none() {
            self.renew_check_at
        } else {
            None
        };
        // Due timers first: a busy socket must not starve heartbeats.
        let now = Instant::now();
        if timer_at.is_some_and(|t| t <= now) {
            return Wake::Timer;
        }
        if connect_at.is_some_and(|t| t <= now) {
            return Wake::Connect;
        }
        let has_out = !self.out.is_empty() && self.sink.is_some();
        let Gateway {
            pending,
            stream,
            sink,
            out,
            closing,
            token_rx,
            ..
        } = self;
        let has_pending = pending.is_some();
        let has_stream = stream.is_some();
        let has_closing = closing.is_some();
        let token_changed = async {
            match token_rx.as_mut() {
                Some(rx) => {
                    if rx.changed().await.is_err() {
                        // The cell is gone; stop watching.
                        *token_rx = None;
                    }
                }
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            biased;
            r = run_pending(pending), if has_pending => Wake::Pending(r),
            r = flush_opt(sink, out), if has_out => Wake::Flushed(r),
            m = next_msg(stream), if has_stream => Wake::Msg(m),
            () = drive_closing(closing), if has_closing => Wake::ClosingDone,
            () = sleep_until_opt(timer_at), if timer_at.is_some() => Wake::Timer,
            () = sleep_until_opt(connect_at), if connect_at.is_some() => Wake::Connect,
            () = sleep_until_opt(renew_at), if renew_at.is_some() => Wake::RenewCheck,
            () = token_changed => Wake::Token,
        }
    }

    fn on_wake(&mut self, wake: Wake) {
        match wake {
            Wake::Pending(r) => {
                self.pending = None;
                self.on_pending(r);
            }
            Wake::Msg(m) => self.on_message(m),
            Wake::Flushed(Ok(())) => {}
            Wake::Flushed(Err(e)) => {
                tracing::debug!(error = %e, "sew-client gateway: write failed");
                self.lost(None, format!("write failed: {e}"));
            }
            Wake::ClosingDone => self.closing = None,
            Wake::Timer => {
                let now = Instant::now();
                for t in TimerKind::ALL {
                    let i = timer_index(t);
                    if self.timers[i].is_some_and(|at| at <= now) {
                        self.timers[i] = None;
                        self.feed(Input::Timer(t));
                    }
                }
            }
            Wake::Connect => {
                self.connect_at = None;
                self.start_connect();
            }
            Wake::RenewCheck => {
                self.renew_check_at = Some(Instant::now() + RENEW_CHECK_INTERVAL);
                let client = self.client.clone();
                self.pending = Some(Box::pin(async move {
                    PendingOut::Checked(client.ensure_fresh().await)
                }));
            }
            Wake::Token => {
                if let Some(t) = self.client.token() {
                    self.feed(Input::TokenChanged(t));
                }
            }
        }
    }

    fn on_pending(&mut self, r: PendingOut) {
        match r {
            PendingOut::Connected { ws, ticket } => {
                if self.stopped {
                    return;
                }
                if let Some(t) = ticket {
                    self.feed(Input::Ticket(t));
                }
                let (sink, stream) = (*ws).split();
                self.sink = Some(sink);
                self.stream = Some(stream);
                self.zstd = None;
                if self.opts.compress {
                    match ZstdStream::new() {
                        Ok(z) => self.zstd = Some(z),
                        Err(e) => tracing::error!(error = %e, "sew-client gateway: zstd decoder"),
                    }
                }
                self.feed(Input::Connected);
            }
            PendingOut::ConnectFailed { code, reason } => {
                tracing::debug!(?code, %reason, "sew-client gateway: connect failed");
                self.feed(Input::Closed { code, reason });
            }
            PendingOut::Refreshed(res) => match res {
                Ok(()) => {
                    if let Some(rx) = self.token_rx.as_mut() {
                        let _ = rx.borrow_and_update();
                    }
                    if let Some(t) = self.client.token() {
                        self.feed(Input::TokenChanged(t));
                    }
                    self.feed(Input::Refreshed);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "sew-client gateway: getting a fresh credential failed");
                    self.feed(Input::RefreshFailed);
                }
            },
            PendingOut::Checked(res) => {
                if let Err(e) = res {
                    tracing::warn!(error = %e, "sew-client gateway: session renew failed");
                }
            }
        }
    }

    /// The socket ended without the machine asking for it.
    fn lost(&mut self, code: Option<u16>, reason: String) {
        self.sink = None;
        self.stream = None;
        self.zstd = None;
        self.out.clear();
        self.feed(Input::Closed { code, reason });
    }

    fn on_message(&mut self, m: Option<WsResult<Message>>) {
        match m {
            None => self.lost(None, "connection closed".to_owned()),
            Some(Err(e)) => self.lost(None, format!("connection error: {e}")),
            Some(Ok(Message::Text(t))) => self.on_bytes(t.as_bytes()),
            Some(Ok(Message::Binary(b))) => {
                let decoded = match self.zstd.as_mut() {
                    Some(z) => z.feed(&b),
                    None => Err(format!(
                        "a binary frame ({} bytes) without compress=zstd-stream",
                        b.len()
                    )),
                };
                match decoded {
                    Ok(bytes) => self.on_bytes(&bytes),
                    Err(e) => self.feed(Input::DecodeError(e)),
                }
            }
            Some(Ok(Message::Close(frame))) => {
                let (code, reason) = match frame {
                    Some(f) => (Some(u16::from(f.code)), f.reason.to_string()),
                    None => (None, "closed without a code".to_owned()),
                };
                self.lost(code, reason);
            }
            Some(Ok(_)) => {}
        }
    }

    fn on_bytes(&mut self, bytes: &[u8]) {
        let frames = match conn::parse_frames(bytes) {
            Ok(f) => f,
            Err(e) => {
                self.feed(Input::DecodeError(e));
                return;
            }
        };
        for frame in frames {
            if self.stream.is_none() {
                // An earlier frame of this batch closed the socket.
                return;
            }
            self.record(FrameDir::In, &frame);
            match ServerOp::decode(&frame) {
                Ok(op) => self.feed(Input::Frame(op)),
                Err(e) => self.feed(Input::DecodeError(e.to_string())),
            }
        }
    }

    async fn flush_now(&mut self) {
        if let Some(sink) = self.sink.as_mut()
            && !self.out.is_empty()
            && let Err(e) = flush(sink, &mut self.out).await
        {
            tracing::debug!(error = %e, "sew-client gateway: write failed");
            self.lost(None, format!("write failed: {e}"));
        }
    }
}

impl Client {
    /// A fresh credential for the gateway after 4004: log in again with the pass source.
    pub(crate) async fn refresh_for_gateway(&self) -> Result<(), ClientError> {
        let cell = self.session_cell()?;
        let generation = cell.generation();
        if !self.relogin_since(generation).await? {
            return Err(ClientError::Unsupported(
                "no pass source to get a fresh credential",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let u = gateway_url("http://127.0.0.1:8470", false).map(|u| u.to_string());
        assert_eq!(
            u.ok().as_deref(),
            Some("ws://127.0.0.1:8470/api/v1/gateway?v=1&encoding=json")
        );
        let u = gateway_url("https://chat.example.com/sew", true).map(|u| u.to_string());
        assert_eq!(
            u.ok().as_deref(),
            Some(
                "wss://chat.example.com/sew/api/v1/gateway?v=1&encoding=json&compress=zstd-stream"
            )
        );
        let t = ticket_url("ws://h/api/v1/gateway?v=1&encoding=json", true).map(|u| u.to_string());
        assert_eq!(
            t.ok().as_deref(),
            Some("ws://h/api/v1/gateway?v=1&encoding=json&compress=zstd-stream")
        );
    }
}
