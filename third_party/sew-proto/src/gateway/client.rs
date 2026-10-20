//! Client-to-server gateway payloads (C7; gaps G12, G13; contracts rev 19).

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use super::{CloseCode, Direction, Frame, Opcode, to_value_or_panic};
use crate::error::{Validate, ValidationError, check_chars, check_known};
use crate::id::{Id, parse_id_str};
use crate::limits::{
    CLIENT_INFO_MAX_CHARS, MEMBER_LIST_RANGE_LEN_MAX, MEMBER_LIST_RANGES_MAX,
    SUBSCRIBE_PRESENCE_MAX, SUBSCRIBE_TYPING_MAX,
};
use crate::token::{SESSION_PREFIX, TICKET_PREFIX, redact};

/// The client's name and version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClientInfo {
    /// Client name, 1–64 characters.
    pub name: String,
    /// Client version, 1–64 characters.
    pub version: String,
}

impl Validate for ClientInfo {
    fn validate(&self) -> Result<(), ValidationError> {
        check_chars("name", &self.name, 1, CLIENT_INFO_MAX_CHARS)?;
        check_chars("version", &self.version, 1, CLIENT_INFO_MAX_CHARS)
    }
}

/// How a socket authenticates: exactly one of a gateway ticket or a session token. On the wire
/// it is the `ticket` or `token` key of the parent object.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum GatewayAuth {
    /// A single-use ticket, `sewtk_…`.
    Ticket(String),
    /// A session token, `sews_…`.
    Token(String),
}

impl fmt::Debug for GatewayAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GatewayAuth::Ticket(t) => write!(f, "Ticket({})", redact(t)),
            GatewayAuth::Token(t) => write!(f, "Token({})", redact(t)),
        }
    }
}

impl GatewayAuth {
    fn from_parts(ticket: Option<String>, token: Option<String>) -> Result<Self, String> {
        match (ticket, token) {
            (Some(t), None) => Ok(GatewayAuth::Ticket(t)),
            (None, Some(t)) => Ok(GatewayAuth::Token(t)),
            (Some(_), Some(_)) => Err("send exactly one of ticket or token, not both".into()),
            (None, None) => Err("send exactly one of ticket or token".into()),
        }
    }

    fn parts(&self) -> (Option<String>, Option<String>) {
        match self {
            GatewayAuth::Ticket(t) => (Some(t.clone()), None),
            GatewayAuth::Token(t) => (None, Some(t.clone())),
        }
    }

    fn check(&self) -> Result<(), ValidationError> {
        match self {
            GatewayAuth::Ticket(t) if !t.starts_with(TICKET_PREFIX) => Err(
                ValidationError::invalid("ticket", "ticket must start with sewtk_"),
            ),
            GatewayAuth::Token(t) if !t.starts_with(SESSION_PREFIX) => Err(
                ValidationError::invalid("token", "token must start with sews_"),
            ),
            _ => Ok(()),
        }
    }
}

/// Op 2 IDENTIFY: authenticate a new gateway session. Send exactly one of `ticket` or `token`.
#[derive(Clone, PartialEq, Eq)]
pub struct IdentifyRequest {
    /// The credential (`ticket` or `token` on the wire).
    pub auth: GatewayAuth,
    /// The client's name and version.
    pub client: ClientInfo,
    /// Capability bits ([`CAP_HIDDEN_SPACES`]); the server ignores unknown bits (gap G12).
    pub capabilities: u32,
}

/// IDENTIFY capability bit 0 `hidden_spaces` (Meet MC1, CC4): READY includes the member's
/// hidden Spaces (the Meetings Space) and their channels. Without it they are left out, so
/// clients that predate hidden Spaces never show them.
pub const CAP_HIDDEN_SPACES: u32 = 1;

impl fmt::Debug for IdentifyRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdentifyRequest")
            .field("auth", &self.auth)
            .field("client", &self.client)
            .field("capabilities", &self.capabilities)
            .finish()
    }
}

/// Op 2 IDENTIFY: authenticate a new gateway session. Send exactly one of `ticket` or `token`.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IdentifyWire {
    /// A single-use gateway ticket, `sewtk_…` (exclusive with `token`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ticket: Option<String>,
    /// A session token, `sews_…` (exclusive with `ticket`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    /// The client's name and version.
    client: ClientInfo,
    /// Capability bits: 1 = `hidden_spaces` (Meet MC1).
    capabilities: u32,
}

impl Serialize for IdentifyRequest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (ticket, token) = self.auth.parts();
        IdentifyWire {
            ticket,
            token,
            client: self.client.clone(),
            capabilities: self.capabilities,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for IdentifyRequest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let w = IdentifyWire::deserialize(deserializer)?;
        Ok(IdentifyRequest {
            auth: GatewayAuth::from_parts(w.ticket, w.token).map_err(serde::de::Error::custom)?,
            client: w.client,
            capabilities: w.capabilities,
        })
    }
}

impl JsonSchema for IdentifyRequest {
    fn schema_name() -> Cow<'static, str> {
        "IdentifyRequest".into()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        IdentifyWire::json_schema(generator)
    }
}

impl Validate for IdentifyRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.auth.check()?;
        self.client.validate().map_err(|e| e.nested("client"))
    }
}

/// Op 4 RESUME: resume a gateway session. Send exactly one of `ticket` or `token`.
#[derive(Clone, PartialEq, Eq)]
pub struct ResumeRequest {
    /// The credential (`ticket` or `token` on the wire).
    pub auth: GatewayAuth,
    /// The session to resume.
    pub session_id: String,
    /// The last `s` the client received.
    pub seq: u64,
}

impl fmt::Debug for ResumeRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResumeRequest")
            .field("auth", &self.auth)
            .field("session_id", &self.session_id)
            .field("seq", &self.seq)
            .finish()
    }
}

/// Op 4 RESUME: resume a gateway session. Send exactly one of `ticket` or `token`.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ResumeWire {
    /// A single-use gateway ticket, `sewtk_…` (exclusive with `token`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ticket: Option<String>,
    /// A session token, `sews_…` (exclusive with `ticket`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    /// The session to resume.
    session_id: String,
    /// The last `s` the client received.
    seq: u64,
}

impl Serialize for ResumeRequest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (ticket, token) = self.auth.parts();
        ResumeWire {
            ticket,
            token,
            session_id: self.session_id.clone(),
            seq: self.seq,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ResumeRequest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let w = ResumeWire::deserialize(deserializer)?;
        Ok(ResumeRequest {
            auth: GatewayAuth::from_parts(w.ticket, w.token).map_err(serde::de::Error::custom)?,
            session_id: w.session_id,
            seq: w.seq,
        })
    }
}

impl JsonSchema for ResumeRequest {
    fn schema_name() -> Cow<'static, str> {
        "ResumeRequest".into()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        ResumeWire::json_schema(generator)
    }
}

impl Validate for ResumeRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.auth.check()?;
        check_chars("session_id", &self.session_id, 1, 128)
    }
}

unit_enum! {
    /// A presence status a client may set.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum PresenceSetStatus {
        /// Online.
        Online = "online",
        /// Idle.
        Idle = "idle",
        /// Do not disturb.
        Dnd = "dnd",
        /// Appear offline.
        Invisible = "invisible",
    }
}

/// Op 3 PRESENCE_SET.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PresenceSetRequest {
    /// The new status.
    pub status: PresenceSetStatus,
}

impl Validate for PresenceSetRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_known("status", self.status)
    }
}

/// A subscription scope: a space id, or the literal `"dms"` (gap G13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubscribeScope {
    /// DMs and group DMs.
    Dms,
    /// One space.
    Space(Id),
}

impl FromStr for SubscribeScope {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "dms" {
            return Ok(SubscribeScope::Dms);
        }
        parse_id_str(s)
            .map(SubscribeScope::Space)
            .ok_or_else(|| format!("invalid subscribe scope {s:?}: expected \"dms\" or a space id"))
    }
}

impl fmt::Display for SubscribeScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SubscribeScope::Dms => f.write_str("dms"),
            SubscribeScope::Space(id) => fmt::Display::fmt(id, f),
        }
    }
}

impl Serialize for SubscribeScope {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SubscribeScope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = Cow::<'de, str>::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for SubscribeScope {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        "SubscribeScope".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^(dms|[1-9][0-9]{0,18})$",
            "description": "A space id, or \"dms\" for DMs and group DMs."
        })
    }
}

/// A member-list subscription: row ranges of one channel's member list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemberListSub {
    /// The channel.
    pub channel_id: Id,
    /// 1–5 inclusive `[lo, hi]` row ranges, each `lo ≤ hi` and `hi - lo < 100`.
    pub ranges: Vec<[u32; 2]>,
}

/// Op 5 SUBSCRIBE: replaces the prior subscription for its scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubscribeRequest {
    /// A space id or `"dms"`.
    pub space_id: SubscribeScope,
    /// Channels to receive typing for (≤ 100).
    #[serde(default)]
    pub typing: Vec<Id>,
    /// Members to receive presence for (≤ 500).
    #[serde(default)]
    pub presence: Vec<Id>,
    /// A member-list subscription.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_list: Option<MemberListSub>,
}

impl SubscribeRequest {
    /// The count limits; failing them closes the socket with 4014.
    pub fn check_limits(&self) -> Result<(), ValidationError> {
        if self.typing.len() > SUBSCRIBE_TYPING_MAX {
            return Err(ValidationError::invalid(
                "typing",
                format!("at most {SUBSCRIBE_TYPING_MAX} typing channels"),
            ));
        }
        if self.presence.len() > SUBSCRIBE_PRESENCE_MAX {
            return Err(ValidationError::invalid(
                "presence",
                format!("at most {SUBSCRIBE_PRESENCE_MAX} presence ids"),
            ));
        }
        if let Some(ml) = &self.member_list {
            if ml.ranges.len() > MEMBER_LIST_RANGES_MAX {
                return Err(ValidationError::invalid(
                    "member_list.ranges",
                    format!("at most {MEMBER_LIST_RANGES_MAX} ranges"),
                ));
            }
            for [lo, hi] in &ml.ranges {
                if hi >= lo && hi - lo >= MEMBER_LIST_RANGE_LEN_MAX {
                    return Err(ValidationError::invalid(
                        "member_list.ranges",
                        format!("a range spans at most {MEMBER_LIST_RANGE_LEN_MAX} rows"),
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Validate for SubscribeRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.check_limits()?;
        if let Some(ml) = &self.member_list {
            if ml.ranges.is_empty() {
                return Err(ValidationError::invalid(
                    "member_list.ranges",
                    "member_list needs at least one range; omit it to unsubscribe",
                ));
            }
            if ml.ranges.iter().any(|[lo, hi]| lo > hi) {
                return Err(ValidationError::invalid(
                    "member_list.ranges",
                    "each range must have lo <= hi",
                ));
            }
        }
        Ok(())
    }
}

/// Op 10 TYPING.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TypingRequest {
    /// The channel.
    pub channel_id: Id,
    /// The thread, when typing a reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Id>,
}

impl Validate for TypingRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Op 11 REAUTH: swap the session token on a live socket. No ack; failure closes 4004.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReauthRequest {
    /// The new credential: a session token (`sews_…`) or a ticket (`sewtk_…`).
    pub token: String,
}

impl fmt::Debug for ReauthRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReauthRequest")
            .field("token", &redact(&self.token))
            .finish()
    }
}

impl Validate for ReauthRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.token.starts_with(SESSION_PREFIX) || self.token.starts_with(TICKET_PREFIX) {
            Ok(())
        } else {
            Err(ValidationError::invalid(
                "token",
                "token must be a session token or a ticket",
            ))
        }
    }
}

/// Op 12 ACTIVITY: the user became active or idle (contracts rev 19).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActivityRequest {
    /// `true` when the user went idle (screen lock, window hidden).
    pub idle: bool,
}

impl Validate for ActivityRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// A decoded client-to-server frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientOp {
    /// Op 1 with the last `s` received.
    Heartbeat(Option<u64>),
    /// Op 2.
    Identify(IdentifyRequest),
    /// Op 3.
    PresenceSet(PresenceSetRequest),
    /// Op 4.
    Resume(ResumeRequest),
    /// Op 5.
    Subscribe(SubscribeRequest),
    /// Op 10.
    Typing(TypingRequest),
    /// Op 11.
    Reauth(ReauthRequest),
    /// Op 12.
    Activity(ActivityRequest),
}

fn decode_payload<T>(d: &Value) -> Result<T, CloseCode>
where
    T: for<'de> Deserialize<'de> + Validate,
{
    let v: T = serde_json::from_value(d.clone()).map_err(|_| CloseCode::DecodeError)?;
    v.validate().map_err(|_| CloseCode::DecodeError)?;
    Ok(v)
}

impl ClientOp {
    /// The opcode.
    pub fn opcode(&self) -> Opcode {
        match self {
            ClientOp::Heartbeat(_) => Opcode::Heartbeat,
            ClientOp::Identify(_) => Opcode::Identify,
            ClientOp::PresenceSet(_) => Opcode::PresenceSet,
            ClientOp::Resume(_) => Opcode::Resume,
            ClientOp::Subscribe(_) => Opcode::Subscribe,
            ClientOp::Typing(_) => Opcode::Typing,
            ClientOp::Reauth(_) => Opcode::Reauth,
            ClientOp::Activity(_) => Opcode::Activity,
        }
    }

    /// Decodes a frame from a client. An unknown or server-only op is 4001; a payload that
    /// does not parse or validate is 4002; a subscription over a limit is 4014.
    pub fn decode(frame: &Frame) -> Result<ClientOp, CloseCode> {
        let op = Opcode::from_u8(frame.op).ok_or(CloseCode::UnknownOpcode)?;
        if op.direction() != Direction::ClientToServer {
            return Err(CloseCode::UnknownOpcode);
        }
        Ok(match op {
            Opcode::Heartbeat => match &frame.d {
                Value::Null => ClientOp::Heartbeat(None),
                Value::Number(n) => {
                    ClientOp::Heartbeat(Some(n.as_u64().ok_or(CloseCode::DecodeError)?))
                }
                _ => return Err(CloseCode::DecodeError),
            },
            Opcode::Identify => ClientOp::Identify(decode_payload(&frame.d)?),
            Opcode::PresenceSet => ClientOp::PresenceSet(decode_payload(&frame.d)?),
            Opcode::Resume => ClientOp::Resume(decode_payload(&frame.d)?),
            Opcode::Subscribe => {
                let req: SubscribeRequest =
                    serde_json::from_value(frame.d.clone()).map_err(|_| CloseCode::DecodeError)?;
                req.check_limits()
                    .map_err(|_| CloseCode::InvalidSubscription)?;
                req.validate().map_err(|_| CloseCode::DecodeError)?;
                ClientOp::Subscribe(req)
            }
            Opcode::Typing => ClientOp::Typing(decode_payload(&frame.d)?),
            Opcode::Reauth => ClientOp::Reauth(decode_payload(&frame.d)?),
            Opcode::Activity => ClientOp::Activity(decode_payload(&frame.d)?),
            Opcode::Dispatch
            | Opcode::Reconnect
            | Opcode::InvalidSession
            | Opcode::Hello
            | Opcode::HeartbeatAck => return Err(CloseCode::UnknownOpcode),
        })
    }

    /// Encodes the op as a client frame (`s` and `t` null).
    pub fn encode(&self) -> Frame {
        let d = match self {
            ClientOp::Heartbeat(s) => match s {
                Some(n) => Value::from(*n),
                None => Value::Null,
            },
            ClientOp::Identify(r) => to_value_or_panic("IdentifyRequest", r),
            ClientOp::PresenceSet(r) => to_value_or_panic("PresenceSetRequest", r),
            ClientOp::Resume(r) => to_value_or_panic("ResumeRequest", r),
            ClientOp::Subscribe(r) => to_value_or_panic("SubscribeRequest", r),
            ClientOp::Typing(r) => to_value_or_panic("TypingRequest", r),
            ClientOp::Reauth(r) => to_value_or_panic("ReauthRequest", r),
            ClientOp::Activity(r) => to_value_or_panic("ActivityRequest", r),
        };
        Frame::new(self.opcode(), d)
    }
}
