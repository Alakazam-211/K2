//! APC extractor for the `k2so:` namespace.
//!
//! vte 0.15 drops APC (Application Program Command) content silently
//! — the `SosPmApcString` state re-feeds bytes through `anywhere()`
//! without surfacing them via any `Perform` callback. APC is a K2SO
//! concept anyway; we pre-filter these sequences out of the byte
//! stream before handing the rest to vte.
//!
//! Wire format per PRD §"APC side-channel namespace":
//!
//! ```text
//! ESC _ k2so:<verb> <json-payload> BEL
//! ```
//!
//! Also accepts ESC-backslash (ST) as a terminator — `ESC _ ... ESC \`
//! is the C0/C1 form some emulators prefer.
//!
//! Supported verbs (the 8 from the PRD):
//!   msg / status / presence / reserve / release / task / tool / tool-result
//!
//! Output: a stream of `ApcChunk`s that interleaves passthrough bytes
//! with extracted events, preserving the relative ordering so
//! `LineMux` can emit frames in the order they appeared in the PTY
//! stream.

use chrono::Utc;
use serde_json::Value;

use crate::awareness::{
    AgentAddress, AgentSignal, PresenceState, Priority, ReservationAction, SignalId,
    SignalKind, TaskPhase, WorkspaceId,
};
use crate::session::SemanticKind;

const ESC: u8 = 0x1B;
const BEL: u8 = 0x07;
const APC_INTRO: u8 = b'_';
const ST_TERM: u8 = b'\\';
const NAMESPACE: &str = "k2so:";

/// One piece of an APC-filtered byte stream. Chunks are emitted in
/// stream order, so consumers get correct interleaving of text and
/// events.
#[derive(Debug)]
pub enum ApcChunk {
    /// Bytes that were NOT part of any APC. Pass these on to the
    /// next parsing stage (e.g. vte's state machine).
    Bytes(Vec<u8>),
    /// A completed `k2so:` APC was extracted at this point in the
    /// stream.
    Event(ApcEvent),
}

/// The semantic payload of a successfully-parsed `k2so:` APC. Either
/// an `AgentSignal` (bound for the Awareness Bus) or a `SemanticEvent`
/// payload (bound for the Session Stream semantic channel).
#[derive(Debug)]
pub enum ApcEvent {
    Signal(AgentSignal),
    Semantic {
        kind: SemanticKind,
        payload: Value,
    },
}

/// Why an APC was discarded instead of emitted. Callers typically log
/// these for observability; they do not surface in the frame stream.
#[derive(Debug)]
pub enum ApcDrop {
    /// APC content didn't start with `k2so:`. Some other program's APC.
    NotOurNamespace,
    /// Our namespace but the verb isn't in the supported list.
    UnknownVerb(String),
    /// Known verb but the payload couldn't be parsed.
    BadPayload { verb: String, err: String },
    /// APC content wasn't valid UTF-8.
    NonUtf8,
}

/// Output of one `ApcExtractor::feed()` call.
#[derive(Debug, Default)]
pub struct ApcOutput {
    pub chunks: Vec<ApcChunk>,
    pub drops: Vec<ApcDrop>,
}

/// Streaming state machine. `feed()` many times across arbitrary
/// chunk boundaries — state persists so APCs that span two PTY
/// reads still resolve correctly.
#[derive(Default)]
pub struct ApcExtractor {
    state: ApcState,
    /// Accumulated content bytes between `ESC _` and `BEL`/`ESC \`.
    buf: Vec<u8>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum ApcState {
    /// Passing bytes through normally.
    #[default]
    Normal,
    /// Just saw ESC; next byte decides whether it's an APC intro.
    AfterEsc,
    /// Between `ESC _` and terminator; accumulating into `buf`.
    InApc,
    /// Inside APC and just saw ESC; looking for `\` (ST).
    InApcAfterEsc,
}

impl ApcExtractor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of bytes. Returns interleaved passthrough /
    /// extracted-event chunks plus any drops.
    pub fn feed(&mut self, bytes: &[u8]) -> ApcOutput {
        let mut chunks: Vec<ApcChunk> = Vec::new();
        let mut drops: Vec<ApcDrop> = Vec::new();
        let mut pending_bytes: Vec<u8> = Vec::new();

        for &byte in bytes {
            match self.state {
                ApcState::Normal => {
                    if byte == ESC {
                        self.state = ApcState::AfterEsc;
                    } else {
                        pending_bytes.push(byte);
                    }
                }
                ApcState::AfterEsc => {
                    if byte == APC_INTRO {
                        // APC opens — flush pending bytes so the
                        // event lands in its correct stream position.
                        if !pending_bytes.is_empty() {
                            chunks.push(ApcChunk::Bytes(std::mem::take(&mut pending_bytes)));
                        }
                        self.buf.clear();
                        self.state = ApcState::InApc;
                    } else {
                        // Not APC — emit the ESC we swallowed plus
                        // this byte as pass-through.
                        pending_bytes.push(ESC);
                        pending_bytes.push(byte);
                        self.state = ApcState::Normal;
                    }
                }
                ApcState::InApc => match byte {
                    BEL => {
                        match parse_apc_content(std::mem::take(&mut self.buf)) {
                            Ok(event) => chunks.push(ApcChunk::Event(event)),
                            Err(drop) => drops.push(drop),
                        }
                        self.state = ApcState::Normal;
                    }
                    ESC => {
                        self.state = ApcState::InApcAfterEsc;
                    }
                    _ => {
                        self.buf.push(byte);
                    }
                },
                ApcState::InApcAfterEsc => {
                    if byte == ST_TERM {
                        // ESC \ = ST — valid terminator.
                        match parse_apc_content(std::mem::take(&mut self.buf)) {
                            Ok(event) => chunks.push(ApcChunk::Event(event)),
                            Err(drop) => drops.push(drop),
                        }
                        self.state = ApcState::Normal;
                    } else {
                        // ESC inside APC without a real ST — treat
                        // both bytes as content and resume.
                        self.buf.push(ESC);
                        self.buf.push(byte);
                        self.state = ApcState::InApc;
                    }
                }
            }
        }

        if !pending_bytes.is_empty() {
            chunks.push(ApcChunk::Bytes(pending_bytes));
        }

        ApcOutput { chunks, drops }
    }
}

fn parse_apc_content(raw: Vec<u8>) -> Result<ApcEvent, ApcDrop> {
    let content = std::str::from_utf8(&raw).map_err(|_| ApcDrop::NonUtf8)?;

    let rest = content.strip_prefix(NAMESPACE).ok_or(ApcDrop::NotOurNamespace)?;

    // Split at first whitespace: "<verb> <json-payload>".
    let (verb, payload_str) = match rest.find(char::is_whitespace) {
        Some(i) => (&rest[..i], rest[i + 1..].trim_start()),
        None => (rest, ""),
    };

    let value: Value = if payload_str.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(payload_str).map_err(|e| ApcDrop::BadPayload {
            verb: verb.to_string(),
            err: e.to_string(),
        })?
    };

    dispatch_verb(verb, value).map_err(|err| match err {
        DispatchErr::Unknown => ApcDrop::UnknownVerb(verb.to_string()),
        DispatchErr::Payload(msg) => ApcDrop::BadPayload {
            verb: verb.to_string(),
            err: msg,
        },
    })
}

enum DispatchErr {
    Unknown,
    Payload(String),
}

/// Parse a `"delivery"` field from an APC payload. Unknown or
/// missing → `Delivery::Live` (the default). Accepts `"live"` and
/// `"inbox"`; other strings fall back to `Live` rather than
/// erroring — see the dispatcher call site for rationale.
fn parse_delivery(value: &Value) -> crate::awareness::Delivery {
    match value.get("delivery").and_then(|v| v.as_str()) {
        Some("inbox") => crate::awareness::Delivery::Inbox,
        Some("live") | None => crate::awareness::Delivery::Live,
        Some(_other) => crate::awareness::Delivery::Live,
    }
}

fn dispatch_verb(verb: &str, value: Value) -> Result<ApcEvent, DispatchErr> {
    // All `Signal` verbs honor an optional top-level `"delivery"`
    // field. Accepted values: `"live"` (default, real-time 1-on-1
    // peer-to-peer) or `"inbox"` (intentional async / notice).
    // Unknown values fall back to `Live` rather than erroring — the
    // sender meant to send something, and silently dropping the
    // signal because of a typo is worse than defaulting to the
    // interrupt path.
    let delivery = parse_delivery(&value);

    match verb {
        "msg" => {
            let to = value.get("to").and_then(|v| v.as_str()).unwrap_or("");
            let text = value.get("text").and_then(|v| v.as_str()).unwrap_or("");
            Ok(ApcEvent::Signal(
                new_signal(
                    AgentAddress::Agent {
                        workspace: WorkspaceId(String::new()),
                        name: to.to_string(),
                    },
                    SignalKind::Msg {
                        text: text.to_string(),
                    },
                )
                .with_delivery(delivery),
            ))
        }
        "status" => {
            let text = value.get("text").and_then(|v| v.as_str()).unwrap_or("");
            Ok(ApcEvent::Signal(
                new_signal(
                    AgentAddress::Broadcast,
                    SignalKind::Status {
                        text: text.to_string(),
                    },
                )
                .with_delivery(delivery),
            ))
        }
        "presence" => {
            let state_str = value.get("state").and_then(|v| v.as_str()).unwrap_or("");
            let state = match state_str {
                "active" => PresenceState::Active,
                "idle" => PresenceState::Idle,
                "away" => PresenceState::Away,
                "stuck" => PresenceState::Stuck,
                other => {
                    return Err(DispatchErr::Payload(format!(
                        "unknown presence state {other:?}"
                    )))
                }
            };
            Ok(ApcEvent::Signal(
                new_signal(
                    AgentAddress::Broadcast,
                    SignalKind::Presence { state },
                )
                .with_delivery(delivery),
            ))
        }
        "reserve" | "release" => {
            let paths = value
                .get("paths")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|p| p.as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let action = if verb == "reserve" {
                ReservationAction::Claim
            } else {
                ReservationAction::Release
            };
            Ok(ApcEvent::Signal(
                new_signal(
                    AgentAddress::Broadcast,
                    SignalKind::Reservation { paths, action },
                )
                .with_delivery(delivery),
            ))
        }
        "task" => {
            let phase_str = value.get("phase").and_then(|v| v.as_str()).unwrap_or("");
            let phase = match phase_str {
                "started" => TaskPhase::Started,
                "done" => TaskPhase::Done,
                "blocked" => TaskPhase::Blocked,
                other => {
                    return Err(DispatchErr::Payload(format!(
                        "unknown task phase {other:?}"
                    )))
                }
            };
            let task_ref = value
                .get("ref")
                .and_then(|v| v.as_str())
                .map(String::from);
            Ok(ApcEvent::Signal(
                new_signal(
                    AgentAddress::Broadcast,
                    SignalKind::TaskLifecycle { phase, task_ref },
                )
                .with_delivery(delivery),
            ))
        }
        "tool" => Ok(ApcEvent::Semantic {
            kind: SemanticKind::ToolCall,
            payload: value,
        }),
        "tool-result" => Ok(ApcEvent::Semantic {
            kind: SemanticKind::ToolResult,
            payload: value,
        }),
        _ => Err(DispatchErr::Unknown),
    }
}

/// Wrap a `SignalKind` into a complete `AgentSignal` with `from` and
/// `session` left as routing placeholders. The Phase 3 routing layer
/// enriches these from session context before egress.
///
/// Delivery defaults to `Live` — APC-emitted signals expect
/// real-time peer-to-peer by default. Senders who want inbox
/// semantics can override via the APC payload's `"delivery":"inbox"`
/// field (parsed in the verb dispatcher), but we haven't threaded
/// that through here yet — E5 adds it when wiring APC ingress.
fn new_signal(to: AgentAddress, kind: SignalKind) -> AgentSignal {
    AgentSignal {
        id: SignalId::new(),
        session: None,
        from: AgentAddress::Broadcast,
        to,
        kind,
        priority: Priority::default(),
        delivery: crate::awareness::Delivery::default(),
        reply_to: None,
        at: Utc::now(),
    }
}
