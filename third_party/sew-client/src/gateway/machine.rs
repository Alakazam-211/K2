//! The gateway state machine (S25 §10.2): no IO, no clock. The driver feeds it inputs with the
//! current monotonic time and performs the actions it returns. All reconnect logic lives here.
//!
//! States: `Idle → Connecting → AwaitHello → Identifying | IdentifyWait | Resuming → Live →
//! Connecting …`, plus `Refreshing` (getting a fresh credential after 4004) and `Stopped`.

use std::time::Duration;

use rand::SeedableRng as _;
use rand::rngs::StdRng;
use sew_proto::gateway::Event;
use sew_proto::gateway::client::{
    ClientInfo, GatewayAuth, IdentifyRequest, PresenceSetRequest, PresenceSetStatus, ReauthRequest,
    ResumeRequest, SubscribeRequest, SubscribeScope, TypingRequest,
};
use sew_proto::gateway::server::{RateLimitedCloseReason, ServerOp};
use sew_proto::gateway::{CLOSE_REASON_RESUMED_ELSEWHERE, CLOSE_REASON_SESSION_EXPIRED, ClientOp};
use sew_proto::id::Id;

use super::{GatewayEvent, NextStep, StopReason};
use crate::retry::Backoff;
use crate::secret::SecretString;

/// No HELLO within this long after connect: close and back off.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// Never two IDENTIFYs with one token closer than this (C7, C16).
pub const IDENTIFY_GATE: Duration = Duration::from_secs(5);
/// One TYPING per channel per this long (C7).
pub const TYPING_INTERVAL: Duration = Duration::from_secs(8);
/// Without a preceding RECONNECT, a 4013 reconnect waits `uniform[0, 15 s)`.
pub const RESTART_JITTER_MAX: Duration = Duration::from_secs(15);

/// A source of uniform samples in `[0, 1)`.
pub trait Jitter: Send {
    /// A sample in `[0, 1)`.
    fn unit(&mut self) -> f64;
}

/// Random jitter from a seeded or entropy-seeded RNG.
#[derive(Debug)]
pub struct RandJitter(StdRng);

impl RandJitter {
    /// From OS entropy.
    pub fn from_entropy() -> RandJitter {
        RandJitter(StdRng::from_entropy())
    }

    /// From a seed (reproducible tests).
    pub fn seeded(seed: u64) -> RandJitter {
        RandJitter(StdRng::seed_from_u64(seed))
    }
}

impl Jitter for RandJitter {
    fn unit(&mut self) -> f64 {
        rand::Rng::r#gen::<f64>(&mut self.0)
    }
}

/// Always the same sample (tests).
#[derive(Debug, Clone, Copy)]
pub struct FixedJitter(pub f64);

impl Jitter for FixedJitter {
    fn unit(&mut self) -> f64 {
        self.0
    }
}

/// A timer the driver runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimerKind {
    /// HELLO must arrive by then.
    HelloTimeout,
    /// The next heartbeat is due.
    Heartbeat,
    /// The identify gate opens.
    IdentifyGate,
}

impl TimerKind {
    /// Every timer.
    pub const ALL: [TimerKind; 3] = [
        TimerKind::HelloTimeout,
        TimerKind::Heartbeat,
        TimerKind::IdentifyGate,
    ];
}

/// What the driver tells the machine.
#[derive(Debug, Clone)]
pub enum Input {
    /// Begin: connect now.
    Start,
    /// The socket is open (after a `Connect`).
    Connected,
    /// A decoded server frame.
    Frame(ServerOp),
    /// The socket is gone, or a connect attempt failed (`code = None`: no close frame).
    Closed {
        /// The close code, when a close frame came.
        code: Option<u16>,
        /// The close reason, or what failed.
        reason: String,
    },
    /// A frame did not decode (a 4002-equivalent local error: re-identify).
    DecodeError(String),
    /// A timer fired.
    Timer(TimerKind),
    /// A single-use ticket for the next connect (ticket mode).
    Ticket(String),
    /// The session token changed (login, renew, re-login).
    TokenChanged(SecretString),
    /// A requested `Refresh` succeeded (the new token came as `TokenChanged`).
    Refreshed,
    /// A requested `Refresh` failed.
    RefreshFailed,
    /// Store a subscription (limits already checked) and send it when live.
    Subscribe(SubscribeRequest),
    /// Store a presence status and send it when live.
    PresenceSet(PresenceSetStatus),
    /// Typing in a channel (throttled here).
    Typing(TypingRequest),
    /// The caller closes the gateway.
    UserClose,
}

/// What the machine asks the driver to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Open a socket after `after` (in ticket mode, mint a ticket first and feed `Ticket`).
    Connect {
        /// The wait before connecting.
        after: Duration,
    },
    /// Send a frame.
    Send(ClientOp),
    /// Hand an event to the caller.
    Emit(GatewayEvent),
    /// (Re)arm a timer.
    SetTimer(TimerKind, Duration),
    /// Disarm a timer.
    CancelTimer(TimerKind),
    /// Close the socket with this code (4000 when reconnecting on purpose, 1000 on close).
    /// The driver then drops the socket and feeds nothing for it.
    CloseSocket(u16),
    /// Get a fresh credential (renew or log in again with the pass source), then feed
    /// `Refreshed` or `RefreshFailed`.
    Refresh,
}

/// How long to wait before the next connect after a close.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitKind {
    /// Full-jitter backoff `uniform[0, min(60 s, 1 s × 2^n))`.
    Backoff,
    /// The last RECONNECT's `after_ms`, else `uniform[0, 15 s)` (4013).
    Restart,
    /// `max(retry_after_ms from the close reason, backoff)` (4008).
    RetryAfterOrBackoff(u64),
    /// Exactly this long (RECONNECT, INVALID_SESSION resumable).
    Exact(Duration),
    /// Connect at once (after a refresh).
    None,
}

/// The step after a close (S25 §10.2 table; contracts rev 22 splits 4004).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosePlan {
    /// Resume the session after the wait.
    Resume(WaitKind),
    /// Forget the session and identify after the wait.
    Identify(WaitKind),
    /// The same step as before (resume if a session is held, else identify) after the wait.
    SameStep(WaitKind),
    /// Drop the last sent subscription, then resume (4014 stays resumable, gap S25-G10).
    DropSubscriptionAndResume(WaitKind),
    /// Get a fresh credential, then the step.
    Refresh(NextStep),
    /// Terminal.
    Stop(StopReason),
}

/// The plan for a close (pure). `fresh_credential` = a pass source exists, or the token already
/// changed since the socket opened.
pub fn close_plan(code: Option<u16>, reason: &str, fresh_credential: bool) -> ClosePlan {
    let Some(code) = code else {
        return ClosePlan::Resume(WaitKind::Backoff);
    };
    match code {
        4000 | 4009 => ClosePlan::Resume(WaitKind::Backoff),
        4013 => ClosePlan::Resume(WaitKind::Restart),
        4001 | 4002 | 4007 | 4010 => ClosePlan::Identify(WaitKind::Backoff),
        4003 => ClosePlan::Identify(WaitKind::Backoff),
        4008 => {
            let ms = serde_json::from_str::<RateLimitedCloseReason>(reason)
                .map(|r| r.retry_after_ms)
                .unwrap_or(0);
            ClosePlan::SameStep(WaitKind::RetryAfterOrBackoff(ms))
        }
        4004 => {
            let then = if reason == CLOSE_REASON_SESSION_EXPIRED {
                NextStep::Resume
            } else {
                NextStep::Identify
            };
            if fresh_credential {
                ClosePlan::Refresh(then)
            } else {
                ClosePlan::Stop(StopReason::AuthFailed)
            }
        }
        4005 if reason == CLOSE_REASON_RESUMED_ELSEWHERE => {
            ClosePlan::Stop(StopReason::ResumedElsewhere)
        }
        4005 => ClosePlan::Stop(StopReason::ClientBug(4005)),
        4011 => ClosePlan::Stop(StopReason::Revoked),
        4012 => ClosePlan::Stop(StopReason::InvalidVersion),
        4014 => ClosePlan::DropSubscriptionAndResume(WaitKind::Backoff),
        // 1000/1001/1006, 4006 and any unknown 4xxx: resume with backoff.
        _ => ClosePlan::Resume(WaitKind::Backoff),
    }
}

/// Machine settings.
#[derive(Debug, Clone)]
pub struct MachineConfig {
    /// Sent in IDENTIFY.
    pub client: ClientInfo,
    /// A pass source exists (4004 can be recovered by a re-login).
    pub can_refresh: bool,
    /// Reconnect backoff (C7: 1 s base, 60 s cap).
    pub backoff: Backoff,
    /// Stop with `GaveUp` after this many failed attempts in a row (`None` = forever).
    pub max_reconnect_attempts: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Connecting,
    AwaitHello,
    IdentifyWait,
    Identifying,
    Resuming,
    Live,
    Refreshing,
    Stopped,
}

/// The driver-visible phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GatewayState {
    /// Not started.
    Idle,
    /// Waiting to connect, connecting, or refreshing a credential.
    Connecting,
    /// Connected; HELLO, IDENTIFY or RESUME in progress.
    Handshaking,
    /// READY or RESUMED received.
    Live,
    /// Terminal.
    Stopped,
}

/// The sans-IO gateway machine.
pub struct Machine {
    cfg: MachineConfig,
    jitter: Box<dyn Jitter>,
    phase: Phase,
    session_id: Option<String>,
    resume_url: Option<String>,
    last_s: Option<u64>,
    attempts: u32,
    hb_interval: Option<Duration>,
    hb_acked: bool,
    resume_buf: Vec<(u64, Event)>,
    subs: Vec<(SubscribeScope, SubscribeRequest)>,
    last_sub: Option<SubscribeScope>,
    presence: Option<PresenceSetStatus>,
    typing_last: Vec<(Id, Duration)>,
    token: Option<SecretString>,
    token_gen: u64,
    gen_at_connect: u64,
    gen_at_handshake: u64,
    ticket: Option<String>,
    last_identify: Option<(Duration, u64)>,
    reconnect_hint: Option<Duration>,
}

impl std::fmt::Debug for Machine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Machine")
            .field("phase", &self.phase)
            .field("session_id", &self.session_id)
            .field("last_s", &self.last_s)
            .field("attempts", &self.attempts)
            .finish_non_exhaustive()
    }
}

impl Machine {
    /// A machine with random jitter.
    pub fn new(cfg: MachineConfig, token: Option<SecretString>) -> Machine {
        Machine::with_jitter(cfg, token, Box::new(RandJitter::from_entropy()))
    }

    /// A machine with a given jitter source (tests).
    pub fn with_jitter(
        cfg: MachineConfig,
        token: Option<SecretString>,
        jitter: Box<dyn Jitter>,
    ) -> Machine {
        Machine {
            cfg,
            jitter,
            phase: Phase::Idle,
            session_id: None,
            resume_url: None,
            last_s: None,
            attempts: 0,
            hb_interval: None,
            hb_acked: true,
            resume_buf: Vec::new(),
            subs: Vec::new(),
            last_sub: None,
            presence: None,
            typing_last: Vec::new(),
            token,
            token_gen: 0,
            gen_at_connect: 0,
            gen_at_handshake: 0,
            ticket: None,
            last_identify: None,
            reconnect_hint: None,
        }
    }

    /// The current session id.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// READY's `resume_url` (advisory, rev 22: resumes go to the ticket's or base URL).
    pub fn resume_url(&self) -> Option<&str> {
        self.resume_url.as_deref()
    }

    /// The last dispatch seq applied.
    pub fn last_seq(&self) -> Option<u64> {
        self.last_s
    }

    /// Failed attempts since the last READY or RESUMED.
    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// The stored subscriptions, in first-stored order.
    pub fn subscriptions(&self) -> impl Iterator<Item = &SubscribeRequest> {
        self.subs.iter().map(|(_, r)| r)
    }

    /// The phase, as the driver shows it.
    pub fn state(&self) -> GatewayState {
        match self.phase {
            Phase::Idle => GatewayState::Idle,
            Phase::Connecting | Phase::Refreshing => GatewayState::Connecting,
            Phase::AwaitHello | Phase::IdentifyWait | Phase::Identifying | Phase::Resuming => {
                GatewayState::Handshaking
            }
            Phase::Live => GatewayState::Live,
            Phase::Stopped => GatewayState::Stopped,
        }
    }

    /// Whether a socket is (being) used: frames and closes matter only then.
    fn connected(&self) -> bool {
        matches!(
            self.phase,
            Phase::AwaitHello
                | Phase::IdentifyWait
                | Phase::Identifying
                | Phase::Resuming
                | Phase::Live
        )
    }

    /// Feeds one input at monotonic time `now`; returns the actions in order.
    pub fn handle(&mut self, now: Duration, input: Input) -> Vec<Action> {
        let mut out = Vec::new();
        if self.phase == Phase::Stopped {
            return out;
        }
        match input {
            Input::Start => {
                if self.phase == Phase::Idle {
                    self.phase = Phase::Connecting;
                    self.gen_at_connect = self.token_gen;
                    out.push(Action::Connect {
                        after: Duration::ZERO,
                    });
                }
            }
            Input::Connected => {
                if self.phase == Phase::Connecting {
                    self.phase = Phase::AwaitHello;
                    self.hb_acked = true;
                    out.push(Action::SetTimer(TimerKind::HelloTimeout, HELLO_TIMEOUT));
                }
            }
            Input::Frame(op) => self.on_frame(now, op, &mut out),
            Input::Closed { code, reason } => {
                if self.connected() || self.phase == Phase::Connecting {
                    let fresh = self.cfg.can_refresh || self.token_gen != self.gen_at_connect;
                    let plan = close_plan(code, &reason, fresh);
                    self.apply(code, reason, plan, &mut out);
                }
            }
            Input::DecodeError(e) => {
                if self.connected() {
                    tracing::warn!(error = %e, "sew-client gateway: undecodable frame; re-identifying");
                    out.push(Action::CloseSocket(4000));
                    self.apply(
                        None,
                        format!("undecodable frame: {e}"),
                        ClosePlan::Identify(WaitKind::Backoff),
                        &mut out,
                    );
                }
            }
            Input::Timer(t) => self.on_timer(now, t, &mut out),
            Input::Ticket(t) => self.ticket = Some(t),
            Input::TokenChanged(t) => {
                self.token = Some(t.clone());
                self.token_gen += 1;
                if self.phase == Phase::Live {
                    out.push(Action::Send(ClientOp::Reauth(ReauthRequest {
                        token: t.expose().to_owned(),
                    })));
                }
            }
            Input::Refreshed => {
                if self.phase == Phase::Refreshing {
                    self.phase = Phase::Connecting;
                    self.gen_at_connect = self.token_gen;
                    out.push(Action::Connect {
                        after: Duration::ZERO,
                    });
                }
            }
            Input::RefreshFailed => {
                if self.phase == Phase::Refreshing {
                    self.stop(StopReason::AuthFailed, &mut out);
                }
            }
            Input::Subscribe(req) => {
                let scope = req.space_id;
                match self.subs.iter_mut().find(|(s, _)| *s == scope) {
                    Some(slot) => slot.1 = req.clone(),
                    None => self.subs.push((scope, req.clone())),
                }
                if self.phase == Phase::Live {
                    self.last_sub = Some(scope);
                    out.push(Action::Send(ClientOp::Subscribe(req)));
                }
            }
            Input::PresenceSet(status) => {
                self.presence = Some(status);
                if self.phase == Phase::Live {
                    out.push(Action::Send(ClientOp::PresenceSet(PresenceSetRequest {
                        status,
                    })));
                }
            }
            Input::Typing(req) => {
                if self.phase != Phase::Live {
                    return out;
                }
                let due = match self
                    .typing_last
                    .iter_mut()
                    .find(|(c, _)| *c == req.channel_id)
                {
                    Some((_, at)) if now.saturating_sub(*at) < TYPING_INTERVAL => false,
                    Some((_, at)) => {
                        *at = now;
                        true
                    }
                    None => {
                        self.typing_last.push((req.channel_id, now));
                        true
                    }
                };
                if due {
                    out.push(Action::Send(ClientOp::Typing(req)));
                }
            }
            Input::UserClose => {
                if self.connected() {
                    out.push(Action::CloseSocket(1000));
                }
                self.stop(StopReason::UserClosed, &mut out);
            }
        }
        out
    }

    fn on_timer(&mut self, now: Duration, t: TimerKind, out: &mut Vec<Action>) {
        match t {
            TimerKind::HelloTimeout => {
                if self.phase == Phase::AwaitHello {
                    out.push(Action::CloseSocket(4000));
                    self.apply(
                        None,
                        "no HELLO within 10 s".to_owned(),
                        ClosePlan::SameStep(WaitKind::Backoff),
                        out,
                    );
                }
            }
            TimerKind::Heartbeat => {
                let beating = matches!(
                    self.phase,
                    Phase::IdentifyWait | Phase::Identifying | Phase::Resuming | Phase::Live
                );
                let Some(interval) = self.hb_interval.filter(|_| beating) else {
                    return;
                };
                if !self.hb_acked {
                    out.push(Action::CloseSocket(4000));
                    self.apply(
                        None,
                        "no HEARTBEAT_ACK since the last heartbeat (zombie socket)".to_owned(),
                        ClosePlan::Resume(WaitKind::Backoff),
                        out,
                    );
                    return;
                }
                out.push(Action::Send(ClientOp::Heartbeat(self.last_s)));
                self.hb_acked = false;
                out.push(Action::SetTimer(TimerKind::Heartbeat, interval));
            }
            TimerKind::IdentifyGate => {
                if self.phase == Phase::IdentifyWait {
                    self.send_identify(now, out);
                }
            }
        }
    }

    fn on_frame(&mut self, now: Duration, op: ServerOp, out: &mut Vec<Action>) {
        if !self.connected() {
            return;
        }
        match op {
            ServerOp::Hello(h) => {
                if self.phase != Phase::AwaitHello {
                    tracing::debug!("sew-client gateway: HELLO outside the handshake; ignored");
                    return;
                }
                out.push(Action::CancelTimer(TimerKind::HelloTimeout));
                let interval = Duration::from_millis(h.heartbeat_interval_ms.max(1));
                self.hb_interval = Some(interval);
                self.hb_acked = true;
                let first = interval.mul_f64(self.jitter.unit().clamp(0.0, 1.0));
                out.push(Action::SetTimer(TimerKind::Heartbeat, first));
                self.handshake(now, out);
            }
            ServerOp::HeartbeatAck => self.hb_acked = true,
            ServerOp::Reconnect(r) => {
                let after = Duration::from_millis(r.after_ms);
                self.reconnect_hint = Some(after);
                out.push(Action::CloseSocket(4000));
                self.apply(
                    Some(4000),
                    "server asked to reconnect".to_owned(),
                    ClosePlan::Resume(WaitKind::Exact(after)),
                    out,
                );
            }
            ServerOp::InvalidSession(inv) => {
                if inv.resumable {
                    let wait = Duration::from_secs(1) + Duration::from_secs(4).mul_f64(self.unit());
                    out.push(Action::CloseSocket(4000));
                    self.apply(
                        Some(4000),
                        "invalid session (resumable)".to_owned(),
                        ClosePlan::Resume(WaitKind::Exact(wait)),
                        out,
                    );
                } else {
                    // C7: forget the session and IDENTIFY on the same socket.
                    self.forget_session();
                    self.identify_or_wait(now, out);
                }
            }
            ServerOp::Dispatch { s, event } => self.on_dispatch(s, event, out),
        }
    }

    fn on_dispatch(&mut self, s: u64, event: Event, out: &mut Vec<Action>) {
        if let Some(l) = self.last_s {
            if s <= l {
                tracing::debug!(
                    s,
                    last_s = l,
                    "sew-client gateway: replayed dispatch dropped"
                );
                return;
            }
            if s > l + 1 {
                tracing::warn!(
                    s,
                    last_s = l,
                    "sew-client gateway: sequence gap; re-identifying (the caller catches up)"
                );
                self.forget_session();
                out.push(Action::CloseSocket(4000));
                self.apply(
                    Some(4000),
                    format!("sequence gap: {s} after {l}"),
                    ClosePlan::Identify(WaitKind::Backoff),
                    out,
                );
                return;
            }
        }
        match event {
            Event::Ready(r) => {
                self.session_id = Some(r.session_id.clone());
                self.resume_url = Some(r.resume_url.clone());
                self.last_s = Some(s);
                self.attempts = 0;
                self.phase = Phase::Live;
                self.resume_buf.clear();
                out.push(Action::Emit(GatewayEvent::Ready(r)));
                self.reauth_if_stale(out);
                // Subscriptions and presence are session state: lost on IDENTIFY (gap S25-G7).
                for (scope, req) in &self.subs {
                    self.last_sub = Some(*scope);
                    out.push(Action::Send(ClientOp::Subscribe(req.clone())));
                }
                if let Some(status) = self.presence {
                    out.push(Action::Send(ClientOp::PresenceSet(PresenceSetRequest {
                        status,
                    })));
                }
            }
            Event::Resumed(_) if self.phase == Phase::Resuming => {
                self.last_s = Some(s);
                self.attempts = 0;
                self.phase = Phase::Live;
                let replayed = u32::try_from(self.resume_buf.len()).unwrap_or(u32::MAX);
                out.push(Action::Emit(GatewayEvent::Resumed { replayed }));
                self.reauth_if_stale(out);
                for (s, event) in self.resume_buf.drain(..) {
                    out.push(Action::Emit(GatewayEvent::Dispatch { s, event }));
                }
            }
            event => {
                self.last_s = Some(s);
                if self.phase == Phase::Resuming {
                    self.resume_buf.push((s, event));
                } else {
                    out.push(Action::Emit(GatewayEvent::Dispatch { s, event }));
                }
            }
        }
    }

    /// The token changed during the handshake: bind the session to the new login now.
    fn reauth_if_stale(&mut self, out: &mut Vec<Action>) {
        if self.token_gen != self.gen_at_handshake
            && let Some(t) = &self.token
        {
            out.push(Action::Send(ClientOp::Reauth(ReauthRequest {
                token: t.expose().to_owned(),
            })));
            self.gen_at_handshake = self.token_gen;
        }
    }

    fn unit(&mut self) -> f64 {
        self.jitter.unit().clamp(0.0, 1.0)
    }

    fn forget_session(&mut self) {
        self.session_id = None;
        self.resume_url = None;
        self.last_s = None;
        self.resume_buf.clear();
    }

    /// The credential for IDENTIFY/RESUME: the minted ticket once, else the token.
    fn auth(&mut self) -> Option<GatewayAuth> {
        if let Some(t) = self.ticket.take() {
            return Some(GatewayAuth::Ticket(t));
        }
        self.token
            .as_ref()
            .map(|t| GatewayAuth::Token(t.expose().to_owned()))
    }

    fn handshake(&mut self, now: Duration, out: &mut Vec<Action>) {
        match (self.session_id.clone(), self.last_s) {
            (Some(session_id), Some(seq)) => {
                let Some(auth) = self.auth() else {
                    out.push(Action::CloseSocket(1000));
                    self.stop(StopReason::AuthFailed, out);
                    return;
                };
                self.resume_buf.clear();
                self.phase = Phase::Resuming;
                self.gen_at_handshake = self.token_gen;
                out.push(Action::Send(ClientOp::Resume(ResumeRequest {
                    auth,
                    session_id,
                    seq,
                })));
            }
            _ => self.identify_or_wait(now, out),
        }
    }

    fn identify_or_wait(&mut self, now: Duration, out: &mut Vec<Action>) {
        if let Some((at, generation)) = self.last_identify
            && generation == self.token_gen
            && now < at + IDENTIFY_GATE
        {
            self.phase = Phase::IdentifyWait;
            out.push(Action::SetTimer(
                TimerKind::IdentifyGate,
                at + IDENTIFY_GATE - now,
            ));
            return;
        }
        self.send_identify(now, out);
    }

    fn send_identify(&mut self, now: Duration, out: &mut Vec<Action>) {
        let Some(auth) = self.auth() else {
            out.push(Action::CloseSocket(1000));
            self.stop(StopReason::AuthFailed, out);
            return;
        };
        self.forget_session();
        self.last_identify = Some((now, self.token_gen));
        self.gen_at_handshake = self.token_gen;
        self.phase = Phase::Identifying;
        out.push(Action::Send(ClientOp::Identify(IdentifyRequest {
            auth,
            client: self.cfg.client.clone(),
            capabilities: 0,
        })));
    }

    fn stop(&mut self, reason: StopReason, out: &mut Vec<Action>) {
        for t in TimerKind::ALL {
            out.push(Action::CancelTimer(t));
        }
        self.phase = Phase::Stopped;
        out.push(Action::Emit(GatewayEvent::Stopped(reason)));
    }

    /// Applies a close plan for the socket that just ended.
    fn apply(&mut self, code: Option<u16>, reason: String, plan: ClosePlan, out: &mut Vec<Action>) {
        for t in TimerKind::ALL {
            out.push(Action::CancelTimer(t));
        }
        self.resume_buf.clear();
        self.hb_acked = true;
        self.ticket = None;
        let wait = match plan {
            ClosePlan::Stop(r) => {
                self.stop(r, out);
                return;
            }
            ClosePlan::Refresh(then) => {
                if then == NextStep::Identify {
                    self.forget_session();
                }
                self.phase = Phase::Refreshing;
                out.push(Action::Emit(GatewayEvent::Disconnected {
                    code,
                    reason,
                    next: then,
                    after: Duration::ZERO,
                }));
                out.push(Action::Refresh);
                return;
            }
            ClosePlan::Identify(w) => {
                self.forget_session();
                w
            }
            ClosePlan::DropSubscriptionAndResume(w) => {
                if let Some(scope) = self.last_sub.take() {
                    self.subs.retain(|(s, _)| *s != scope);
                }
                w
            }
            ClosePlan::Resume(w) | ClosePlan::SameStep(w) => w,
        };
        let n = self.attempts;
        self.attempts = self.attempts.saturating_add(1);
        if let Some(max) = self.cfg.max_reconnect_attempts
            && self.attempts > max
        {
            self.stop(StopReason::GaveUp, out);
            return;
        }
        let backoff = |m: &mut Machine| {
            let u = m.unit();
            m.cfg.backoff.delay(n, u)
        };
        let after = match wait {
            WaitKind::Backoff => backoff(self),
            WaitKind::Restart => match self.reconnect_hint.take() {
                Some(d) => d,
                None => RESTART_JITTER_MAX.mul_f64(self.unit()),
            },
            WaitKind::RetryAfterOrBackoff(ms) => backoff(self).max(Duration::from_millis(ms)),
            WaitKind::Exact(d) => d,
            WaitKind::None => Duration::ZERO,
        };
        let next = if self.session_id.is_some() && self.last_s.is_some() {
            NextStep::Resume
        } else {
            NextStep::Identify
        };
        out.push(Action::Emit(GatewayEvent::Disconnected {
            code,
            reason,
            next,
            after,
        }));
        self.phase = Phase::Connecting;
        self.gen_at_connect = self.token_gen;
        out.push(Action::Connect { after });
    }
}
