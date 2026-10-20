//! The websocket gateway (C7; gaps G11–G14; contracts revs 12, 19, 22).
//!
//! `GET /api/v1/gateway?v=1&encoding=json[&compress=zstd-stream]`. Every frame is
//! `{"op": <int>, "d": <any>, "s": <int|null>, "t": <string|null>}`; `s` and `t` are set only on
//! op 0.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Validate, ValidationError, check_known};

pub mod client;
pub mod events;
pub mod server;

pub use client::ClientOp;
pub use events::{Event, EventName};
pub use server::ServerOp;

/// The gateway protocol version clients send as `v`.
pub const GATEWAY_VERSION: u32 = 1;

/// A gateway frame. The server always emits all four keys; client frames may omit `s` and `t`.
/// `op` stays a raw number so an unknown op can close the socket with 4001.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Frame {
    /// The opcode.
    pub op: u8,
    /// The payload.
    #[serde(default)]
    pub d: Value,
    /// The session seq; set only on dispatch (op 0).
    #[serde(default)]
    pub s: Option<u64>,
    /// The event name; set only on dispatch (op 0).
    #[serde(default)]
    pub t: Option<String>,
}

impl Frame {
    /// A non-dispatch frame: `s` and `t` are null.
    pub fn new(op: Opcode, d: Value) -> Self {
        Frame {
            op: op.as_u8(),
            d,
            s: None,
            t: None,
        }
    }
}

pub(crate) fn to_value_or_panic<T: Serialize>(what: &str, v: &T) -> Value {
    match serde_json::to_value(v) {
        Ok(v) => v,
        // Only maps with non-string keys fail, and no gateway type has one.
        Err(e) => panic!("sew-proto: serializing {what} failed: {e}"),
    }
}

unit_enum! {
    /// Gateway payload encoding; only `json` in v1.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum Encoding {
        /// JSON text frames.
        Json = "json",
    }
}

unit_enum! {
    /// Gateway transport compression.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum Compression {
        /// One zstd context per connection; server frames are binary flushed zstd blocks.
        ZstdStream = "zstd-stream",
    }
}

/// Query of `GET /api/v1/gateway`. Any other `v` closes with 4012.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GatewayQuery {
    /// Protocol version; must be 1.
    pub v: u32,
    /// Payload encoding; must be `json`.
    pub encoding: Encoding,
    /// Optional transport compression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compress: Option<Compression>,
}

impl Validate for GatewayQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.v != GATEWAY_VERSION {
            return Err(ValidationError::invalid(
                "v",
                format!(
                    "gateway version {} is not supported; use {GATEWAY_VERSION}",
                    self.v
                ),
            ));
        }
        check_known("encoding", self.encoding)?;
        if let Some(c) = self.compress {
            check_known("compress", c)?;
        }
        Ok(())
    }
}

/// Which way an opcode travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Server to client.
    ServerToClient,
    /// Client to server.
    ClientToServer,
}

/// A gateway opcode (C7; op 12 from contracts rev 19).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Opcode {
    /// 0, S→C: an event.
    Dispatch,
    /// 1, C→S: heartbeat with the last `s` received.
    Heartbeat,
    /// 2, C→S: authenticate a new session.
    Identify,
    /// 3, C→S: set presence.
    PresenceSet,
    /// 4, C→S: resume a session.
    Resume,
    /// 5, C→S: subscribe to typing, presence and member lists.
    Subscribe,
    /// 6, S→C: reconnect and resume after a delay.
    Reconnect,
    /// 7, S→C: the session cannot be resumed.
    InvalidSession,
    /// 8, S→C: first frame; heartbeat interval and limits.
    Hello,
    /// 9, S→C: heartbeat acknowledged.
    HeartbeatAck,
    /// 10, C→S: typing in a channel.
    Typing,
    /// 11, C→S: swap the session token on a live socket.
    Reauth,
    /// 12, C→S: the user became active or idle (contracts rev 19).
    Activity,
}

impl Opcode {
    /// Every opcode in number order.
    pub const ALL: &'static [Opcode] = &[
        Opcode::Dispatch,
        Opcode::Heartbeat,
        Opcode::Identify,
        Opcode::PresenceSet,
        Opcode::Resume,
        Opcode::Subscribe,
        Opcode::Reconnect,
        Opcode::InvalidSession,
        Opcode::Hello,
        Opcode::HeartbeatAck,
        Opcode::Typing,
        Opcode::Reauth,
        Opcode::Activity,
    ];

    /// The wire number.
    pub const fn as_u8(self) -> u8 {
        match self {
            Opcode::Dispatch => 0,
            Opcode::Heartbeat => 1,
            Opcode::Identify => 2,
            Opcode::PresenceSet => 3,
            Opcode::Resume => 4,
            Opcode::Subscribe => 5,
            Opcode::Reconnect => 6,
            Opcode::InvalidSession => 7,
            Opcode::Hello => 8,
            Opcode::HeartbeatAck => 9,
            Opcode::Typing => 10,
            Opcode::Reauth => 11,
            Opcode::Activity => 12,
        }
    }

    /// Looks up a wire number.
    pub const fn from_u8(n: u8) -> Option<Opcode> {
        Some(match n {
            0 => Opcode::Dispatch,
            1 => Opcode::Heartbeat,
            2 => Opcode::Identify,
            3 => Opcode::PresenceSet,
            4 => Opcode::Resume,
            5 => Opcode::Subscribe,
            6 => Opcode::Reconnect,
            7 => Opcode::InvalidSession,
            8 => Opcode::Hello,
            9 => Opcode::HeartbeatAck,
            10 => Opcode::Typing,
            11 => Opcode::Reauth,
            12 => Opcode::Activity,
            _ => return None,
        })
    }

    /// The C7 name, `UPPER_SNAKE`.
    pub const fn name(self) -> &'static str {
        match self {
            Opcode::Dispatch => "DISPATCH",
            Opcode::Heartbeat => "HEARTBEAT",
            Opcode::Identify => "IDENTIFY",
            Opcode::PresenceSet => "PRESENCE_SET",
            Opcode::Resume => "RESUME",
            Opcode::Subscribe => "SUBSCRIBE",
            Opcode::Reconnect => "RECONNECT",
            Opcode::InvalidSession => "INVALID_SESSION",
            Opcode::Hello => "HELLO",
            Opcode::HeartbeatAck => "HEARTBEAT_ACK",
            Opcode::Typing => "TYPING",
            Opcode::Reauth => "REAUTH",
            Opcode::Activity => "ACTIVITY",
        }
    }

    /// Which way the opcode travels.
    pub const fn direction(self) -> Direction {
        match self {
            Opcode::Dispatch
            | Opcode::Reconnect
            | Opcode::InvalidSession
            | Opcode::Hello
            | Opcode::HeartbeatAck => Direction::ServerToClient,
            Opcode::Heartbeat
            | Opcode::Identify
            | Opcode::PresenceSet
            | Opcode::Resume
            | Opcode::Subscribe
            | Opcode::Typing
            | Opcode::Reauth
            | Opcode::Activity => Direction::ClientToServer,
        }
    }
}

/// What a client does after a close code (C7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClientAction {
    /// Resume the session.
    Resume,
    /// Identify a new session.
    Reidentify,
    /// Identify (the socket never authenticated).
    Identify,
    /// Get a new session and do not retry with the old credential.
    NewSessionNoRetry,
    /// A client bug; fix the client.
    FixClient,
    /// Wait `retry_after_ms` from the close reason, then retry.
    WaitRetryAfter,
    /// Stop and drop local data (session revoked or account removed).
    StopDropLocalData,
    /// Stop.
    Stop,
    /// Resume after the delay in the close reason.
    ResumeAfterDelay,
    /// Fix the subscription and resend it.
    FixAndResend,
    /// Get a fresh credential (renew or ticket) and resume; the session is parked
    /// (4004 with reason `session_expired`, contracts revs 12 and 22).
    RefreshAndResume,
}

/// Close reason: the bound login session expired; the gateway session is parked (4004).
pub const CLOSE_REASON_SESSION_EXPIRED: &str = "session_expired";
/// Close reason: another socket resumed this session (4005). Do not auto-resume.
pub const CLOSE_REASON_RESUMED_ELSEWHERE: &str = "resumed_elsewhere";

/// A gateway close code (C7). 4006 is unassigned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CloseCode {
    /// 4000: unknown error.
    UnknownError,
    /// 4001: unknown opcode.
    UnknownOpcode,
    /// 4002: decode error.
    DecodeError,
    /// 4003: not authenticated.
    NotAuthenticated,
    /// 4004: authentication failed (reason `session_expired` when the login expired).
    AuthenticationFailed,
    /// 4005: already authenticated (reason `resumed_elsewhere` when another socket took over).
    AlreadyAuthenticated,
    /// 4007: invalid seq.
    InvalidSeq,
    /// 4008: rate limited; the reason is [`server::RateLimitedCloseReason`] JSON.
    RateLimited,
    /// 4009: session timed out.
    SessionTimedOut,
    /// 4010: slow consumer.
    SlowConsumer,
    /// 4011: session revoked or account removed.
    SessionRevoked,
    /// 4012: invalid API version.
    InvalidApiVersion,
    /// 4013: server restarting.
    ServerRestarting,
    /// 4014: invalid subscription.
    InvalidSubscription,
}

impl CloseCode {
    /// Every close code in number order.
    pub const ALL: &'static [CloseCode] = &[
        CloseCode::UnknownError,
        CloseCode::UnknownOpcode,
        CloseCode::DecodeError,
        CloseCode::NotAuthenticated,
        CloseCode::AuthenticationFailed,
        CloseCode::AlreadyAuthenticated,
        CloseCode::InvalidSeq,
        CloseCode::RateLimited,
        CloseCode::SessionTimedOut,
        CloseCode::SlowConsumer,
        CloseCode::SessionRevoked,
        CloseCode::InvalidApiVersion,
        CloseCode::ServerRestarting,
        CloseCode::InvalidSubscription,
    ];

    /// The websocket close code.
    pub const fn as_u16(self) -> u16 {
        match self {
            CloseCode::UnknownError => 4000,
            CloseCode::UnknownOpcode => 4001,
            CloseCode::DecodeError => 4002,
            CloseCode::NotAuthenticated => 4003,
            CloseCode::AuthenticationFailed => 4004,
            CloseCode::AlreadyAuthenticated => 4005,
            CloseCode::InvalidSeq => 4007,
            CloseCode::RateLimited => 4008,
            CloseCode::SessionTimedOut => 4009,
            CloseCode::SlowConsumer => 4010,
            CloseCode::SessionRevoked => 4011,
            CloseCode::InvalidApiVersion => 4012,
            CloseCode::ServerRestarting => 4013,
            CloseCode::InvalidSubscription => 4014,
        }
    }

    /// Looks up a close code. `None` for 4006 and anything unassigned.
    pub const fn from_u16(n: u16) -> Option<CloseCode> {
        Some(match n {
            4000 => CloseCode::UnknownError,
            4001 => CloseCode::UnknownOpcode,
            4002 => CloseCode::DecodeError,
            4003 => CloseCode::NotAuthenticated,
            4004 => CloseCode::AuthenticationFailed,
            4005 => CloseCode::AlreadyAuthenticated,
            4007 => CloseCode::InvalidSeq,
            4008 => CloseCode::RateLimited,
            4009 => CloseCode::SessionTimedOut,
            4010 => CloseCode::SlowConsumer,
            4011 => CloseCode::SessionRevoked,
            4012 => CloseCode::InvalidApiVersion,
            4013 => CloseCode::ServerRestarting,
            4014 => CloseCode::InvalidSubscription,
            _ => return None,
        })
    }

    /// The C7 client action for the code alone.
    pub const fn client_action(self) -> ClientAction {
        match self {
            CloseCode::UnknownError => ClientAction::Resume,
            CloseCode::UnknownOpcode => ClientAction::Reidentify,
            CloseCode::DecodeError => ClientAction::Reidentify,
            CloseCode::NotAuthenticated => ClientAction::Identify,
            CloseCode::AuthenticationFailed => ClientAction::NewSessionNoRetry,
            CloseCode::AlreadyAuthenticated => ClientAction::FixClient,
            CloseCode::InvalidSeq => ClientAction::Reidentify,
            CloseCode::RateLimited => ClientAction::WaitRetryAfter,
            CloseCode::SessionTimedOut => ClientAction::Resume,
            CloseCode::SlowConsumer => ClientAction::Reidentify,
            CloseCode::SessionRevoked => ClientAction::StopDropLocalData,
            CloseCode::InvalidApiVersion => ClientAction::Stop,
            CloseCode::ServerRestarting => ClientAction::ResumeAfterDelay,
            CloseCode::InvalidSubscription => ClientAction::FixAndResend,
        }
    }

    /// The client action given the close reason too: 4004 with `session_expired` means the
    /// session is parked and resumable with a fresh credential (contracts revs 12, 22).
    pub fn client_action_for(self, reason: &str) -> ClientAction {
        match (self, reason) {
            (CloseCode::AuthenticationFailed, CLOSE_REASON_SESSION_EXPIRED) => {
                ClientAction::RefreshAndResume
            }
            _ => self.client_action(),
        }
    }
}

impl fmt::Display for CloseCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({self:?})", self.as_u16())
    }
}
