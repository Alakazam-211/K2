//! The Live socket's frames (`WS /cli/docs/live`, [`super::live`]): JSON in
//! and out, for the kit's hunk channel (the vendored client's `WIRE.md`
//! and `src/wire.ts`) plus K2's own handshake and close frames.
//!
//! The kit's Rust types carry no serde, so the shapes are written here, field
//! for field as `wire.ts` names them. Offsets are UTF-8 bytes, versions are
//! the doc's `Seq`.
//!
//! **Provisional (L2, 2026-10-30).** The kit's `WIRE.md` doesn't yet pin the
//! frames below; K2 Cores is closing its gaps G1–G10 in one client-package +
//! WIRE.md change. Until then these follow the L2 transport spec draft v0 and
//! its addendum. Each provisional shape is marked with its gap:
//!
//! | Gap | Frame(s) here |
//! |---|---|
//! | G1 | `welcome`, `snapshot` (request and reply) |
//! | G2 | `closed {reason, retry, code?, message?, error?, version?, written?, core?}`, `error {error}` |
//! | G3 | the own-edit filter (by version, in [`super::live`]) |
//! | G6 | `cursor` is applied only at the doc's current version, else dropped |
//! | G7 | every encoding below (no golden vectors from the kit yet) |
//! | G8 | `{"t":"viewing","visible"}` (also `{"type":"visible"}`), `view {root?, first, last}`; no app-level ping |
//! | G9 | `undo_result {op, outcome, steps, version}` for `undo` / `redo` |
//! | G10 | the 4 MiB client-frame cap ([`FRAME_CAP`]) |

use serde_json::{json, Value};

use k2_mpc_base::errors::{ErrorReport, Refusal};
use k2_mpc_base::session::{
    ChangeEvent, CursorAt, LeaveReason, Participant, ParticipantKind, Rights, Role, RosterEntry, RosterEvent,
    RosterMember, SessionError, UndoOutcome, Who,
};
use k2_mpc_base::text::Hunk;

/// The largest client frame K2 takes (G10): a whole 1 MiB paste as JSON
/// with room, and a 4 MiB doc's worth of hunks. A text frame over it, up to
/// [`HARD_CAP`], is answered (`refused` for an op, else `error`) with the
/// kit's `MPC-E113` and the socket stays open; past [`HARD_CAP`] the socket
/// closes (`closed {reason: "cut_off"}`, code 1009). Server frames aren't
/// capped: a `welcome` or `snapshot` carries the whole text (the Markdown
/// and text packs hold at most 4 MiB, JSON-escaped).
pub const FRAME_CAP: usize = 4 << 20;

/// The WebSocket layer's own limit (message and frame): past it the read
/// fails and the socket is closed.
pub const HARD_CAP: usize = 8 << 20;

/// Why a Live socket ended (the spec's `CloseReason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// The client closed the tab ([`super::live`] calls `leave`).
    Left,
    Idle,
    Kicked,
    RightsLost,
    Deleted,
    Moved,
    /// The daemon handed off or is stopping: the next one serves.
    Released,
    /// The socket fell behind the doc (the kit's subscriber queue), or a
    /// frame was over [`HARD_CAP`]: reconnect and resume.
    CutOff,
    /// The file can't be live (never-live, a pack refused it, not a text doc).
    Declined,
    NotFound,
    /// The caller may not read this doc.
    Auth,
    Network,
    /// K2 closed it for good (its core was switched off, PRD
    /// multiplayer-cores-settings §6.1): no retry; open the file the usual way.
    Closed,
}

impl CloseReason {
    pub fn as_str(self) -> &'static str {
        match self {
            CloseReason::Left => "left",
            CloseReason::Idle => "idle",
            CloseReason::Kicked => "kicked",
            CloseReason::RightsLost => "rights_lost",
            CloseReason::Deleted => "deleted",
            CloseReason::Moved => "moved",
            CloseReason::Released => "released",
            CloseReason::CutOff => "cut_off",
            CloseReason::Declined => "declined",
            CloseReason::NotFound => "not_found",
            CloseReason::Auth => "auth",
            CloseReason::Network => "network",
            CloseReason::Closed => "closed",
        }
    }

    /// Whether the component should reconnect (with `resume`). The daemon
    /// says so in the frame, so both sides agree.
    pub fn retry(self) -> bool {
        matches!(
            self,
            CloseReason::Idle | CloseReason::Moved | CloseReason::Released | CloseReason::CutOff | CloseReason::Network
        )
    }

    /// From the kit's leave reason.
    pub fn from_leave(r: LeaveReason) -> CloseReason {
        match r {
            LeaveReason::Left => CloseReason::Left,
            LeaveReason::Idle => CloseReason::Idle,
            LeaveReason::Kicked => CloseReason::Kicked,
            LeaveReason::RightsLost => CloseReason::RightsLost,
        }
    }
}

/// WebSocket close codes. The `closed` frame before them is the answer;
/// the code is for a client that missed it.
pub mod code {
    pub const NORMAL: u16 = 1000;
    /// Frame over [`super::HARD_CAP`].
    pub const TOO_BIG: u16 = 1009;
    /// The core was switched off (PRD multiplayer-cores-settings §6.1).
    pub const CORE_OFF: u16 = 4010;
}

/// The final frame before a close (G2).
#[derive(Debug, Clone, PartialEq)]
pub struct Closed {
    pub reason: CloseReason,
    /// K2's own code (`core_off`, `secret`, `file_not_found`...), when K2
    /// (not the kit) refused.
    pub code: Option<String>,
    pub message: Option<String>,
    /// The kit's `ErrorReport` JSON, when the kit refused.
    pub error: Option<Value>,
    /// The doc's version when it closed (and `written`: every change up to
    /// it is in the file), for a core switched off.
    pub version: Option<u64>,
    pub written: Option<bool>,
    pub core: Option<String>,
    /// The WebSocket close code that follows.
    pub ws_code: u16,
}

impl Closed {
    pub fn new(reason: CloseReason) -> Self {
        Closed { reason, code: None, message: None, error: None, version: None, written: None, core: None, ws_code: code::NORMAL }
    }

    /// K2's own refusal: a code and a message.
    pub fn k2(reason: CloseReason, code: &str, message: impl Into<String>) -> Self {
        Closed { code: Some(code.to_string()), message: Some(message.into()), ..Closed::new(reason) }
    }

    /// The kit's refusal.
    pub fn kit(reason: CloseReason, e: &SessionError) -> Self {
        Closed { error: Some(error_json(&e.report())), ..Closed::new(reason) }
    }

    pub fn json(&self) -> Value {
        let mut v = json!({ "type": "closed", "reason": self.reason.as_str(), "retry": self.reason.retry() });
        let o = v.as_object_mut().expect("object");
        if let Some(c) = &self.code {
            o.insert("code".into(), json!(c));
        }
        if let Some(m) = &self.message {
            o.insert("message".into(), json!(m));
        }
        if let Some(e) = &self.error {
            o.insert("error".into(), e.clone());
        }
        if let Some(n) = self.version {
            o.insert("version".into(), json!(n));
        }
        if let Some(w) = self.written {
            o.insert("written".into(), json!(w));
        }
        if let Some(c) = &self.core {
            o.insert("core".into(), json!(c));
        }
        v
    }
}

/// The kit's `ErrorReport` as JSON: `{id, exit, message, next, rule, details}`.
pub fn error_json(r: &ErrorReport) -> Value {
    serde_json::from_str(&r.to_json()).unwrap_or_else(|e| json!({ "id": "MPC-E000", "message": format!("unreadable error report: {e}") }))
}

/// The kit's `MPC-E113` (`bad_edit`) for a frame K2 refuses.
pub fn bad_edit(msg: impl Into<String>) -> Value {
    error_json(&SessionError::BadEdit(msg.into()).report())
}

pub fn hunk_json(h: &Hunk) -> Value {
    json!({ "start": h.start, "end": h.end, "insert": h.insert })
}

pub fn hunks_json(hs: &[Hunk]) -> Value {
    Value::Array(hs.iter().map(hunk_json).collect())
}

/// The kit's `ChangeEvent`: `{version, pid, client, via, root, hunks, at_ms, on_behalf_of}`.
pub fn change_json(e: &ChangeEvent) -> Value {
    json!({
        "version": e.version,
        "pid": e.pid,
        "client": e.client,
        "via": e.via,
        "root": e.root,
        "hunks": hunks_json(&e.hunks),
        "at_ms": e.at_ms,
        "on_behalf_of": e.on_behalf_of,
    })
}

/// `"edit"`, `"read"` or `"none"`.
pub fn rights_word(r: &Rights) -> &'static str {
    match (r.read, r.edit) {
        (true, true) => "edit",
        (true, false) => "read",
        _ => "none",
    }
}

fn who(w: &Who) -> String {
    w.to_string()
}

fn kind_str(k: ParticipantKind) -> &'static str {
    match k {
        ParticipantKind::Person => "person",
        ParticipantKind::Agent => "agent",
        ParticipantKind::App => "app",
        ParticipantKind::External => "external",
        ParticipantKind::Restore => "restore",
        ParticipantKind::Import => "import",
    }
}

fn role_str(r: Role) -> &'static str {
    match r {
        Role::Editor => "editor",
        Role::Viewer => "viewer",
    }
}

fn leave_str(r: LeaveReason) -> &'static str {
    CloseReason::from_leave(r).as_str()
}

/// A caret as `CaretAt`: `{root, anchor, head, version?}` (bytes).
fn caret_json(c: &CursorAt, version: Option<u64>) -> Value {
    let mut v = json!({ "root": c.cursor.root, "anchor": c.cursor.anchor, "head": c.cursor.head });
    if let Some(n) = version {
        v["version"] = json!(n);
    }
    v
}

fn lines_json(l: &k2_mpc_base::session::LineRange) -> Value {
    json!({ "root": l.root, "first": l.first, "last": l.last })
}

fn participant_json(p: &Participant) -> Value {
    json!({ "name": p.name, "colour": p.colour, "kind": kind_str(p.kind), "via": p.via })
}

/// A roster entry as `wire.ts` has it: `{handle, name, colour, kind, role,
/// away, cursor, rights}` (carets at `version`, the snapshot's).
fn entry_json(handle: String, e: &RosterEntry, rights: &Rights, version: u64) -> Value {
    json!({
        "handle": handle,
        "name": e.participant.name,
        "colour": e.participant.colour,
        "kind": kind_str(e.participant.kind),
        "role": role_str(e.role),
        "away": e.away,
        "cursor": e.cursor.as_ref().map(|c| caret_json(c, Some(version))),
        "rights": rights_word(rights),
    })
}

fn member_json(m: &RosterMember, version: u64) -> Value {
    entry_json(who(&m.who), &m.entry, &m.rights, version)
}

/// One `RosterEvent` (K5) as `roster_event.event`, tagged by `kind`. `None`
/// for an event this K2 doesn't know (the kit's enum is non-exhaustive).
pub fn roster_event_json(ev: &RosterEvent) -> Option<Value> {
    Some(match ev {
        RosterEvent::Snapshot { version, members, watching, .. } => json!({
            "kind": "snapshot",
            "version": version,
            "members": members.iter().map(|m| member_json(m, *version)).collect::<Vec<_>>(),
            "watching": watching,
        }),
        RosterEvent::Joined { who: w, participant, rights, .. } => json!({
            "kind": "joined", "who": who(w), "participant": participant_json(participant), "rights": rights_word(rights),
        }),
        RosterEvent::Left { who: w, reason, .. } => json!({ "kind": "left", "who": who(w), "reason": leave_str(*reason) }),
        RosterEvent::Rights { who: w, rights, .. } => json!({ "kind": "rights", "who": who(w), "rights": rights_word(rights) }),
        RosterEvent::Role { who: w, role, .. } => json!({ "kind": "role", "who": who(w), "role": role_str(*role) }),
        RosterEvent::Away { who: w, away, .. } => json!({ "kind": "away", "who": who(w), "away": away }),
        RosterEvent::Cursor { who: w, cursor, version, .. } => json!({
            "kind": "cursor", "who": who(w), "cursor": cursor.as_ref().map(|c| caret_json(c, Some(*version))),
        }),
        RosterEvent::ClaimStarted { who: w, claim, lines, note, hold, for_edit, expires_ms, version, .. } => json!({
            "kind": "claim_started", "who": who(w), "claim": claim.to_string(), "lines": lines_json(lines),
            "note": note, "hold": hold, "for_edit": for_edit, "expires_ms": expires_ms, "version": version,
        }),
        RosterEvent::ClaimEnded { who: w, claim, how, lines, version, .. } => json!({
            "kind": "claim_ended", "who": who(w), "claim": claim.to_string(), "how": how.as_str(),
            "lines": lines_json(lines), "version": version,
        }),
        RosterEvent::Note { to, from, kind, lines, claim, .. } => json!({
            "kind": "note", "to": who(to), "from": who(from), "note": kind.as_str(), "lines": lines_json(lines),
            "claim": claim.as_ref().map(|c| c.to_string()),
        }),
        RosterEvent::Watching { count, .. } => json!({ "kind": "watching", "count": count }),
        _ => return None,
    })
}

/// G9 (provisional): the reply to `undo` / `redo`. The change itself
/// arrives as a `change` (before this frame).
pub fn undo_result_json(op: u64, out: &UndoOutcome, version: u64) -> Value {
    let (outcome, steps) = match out {
        UndoOutcome::Applied { steps, .. } => ("applied", *steps),
        UndoOutcome::Conflicts { .. } => ("conflicts", 0),
        UndoOutcome::Blocked { .. } => ("blocked", 0),
        UndoOutcome::Dropped { .. } => ("dropped", 0),
        UndoOutcome::Nothing => ("nothing", 0),
        UndoOutcome::HistoryLost => ("history_lost", 0),
        UndoOutcome::ClearedByPurge => ("cleared_by_purge", 0),
        _ => ("other", 0),
    };
    json!({ "type": "undo_result", "op": op, "outcome": outcome, "steps": steps, "version": version })
}

/// A frame from the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientFrame {
    Edit { op: u64, root: String, base: u64, hunks: Vec<Hunk>, group: Option<String> },
    Since { op: u64, version: u64 },
    Cursor { root: String, version: u64, anchor: usize, head: usize },
    Undo { op: u64 },
    Redo { op: u64 },
    /// G1: the whole text now (after `MPC-E112`, `onReload`).
    Snapshot { op: u64 },
    /// G8: the Thread's `{"t":"viewing","visible"}` (or `{"type":"visible"}`).
    Visible(bool),
    /// G8: the lines in view (1-based, inclusive).
    View { root: Option<String>, first: usize, last: usize },
}

/// Why a client frame was refused: its `op` when it had one (the reply is
/// then `refused`, else `error`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadFrame {
    pub op: Option<u64>,
    pub message: String,
}

fn u64_field(o: &serde_json::Map<String, Value>, k: &str) -> Result<u64, String> {
    o.get(k).and_then(Value::as_u64).ok_or_else(|| format!("`{k}` must be a whole number"))
}

fn str_field(o: &serde_json::Map<String, Value>, k: &str) -> Result<String, String> {
    o.get(k).and_then(Value::as_str).map(str::to_string).ok_or_else(|| format!("`{k}` must be a string"))
}

/// Hunks as objects `{start, end, insert}` (the vectors' array form is not
/// a wire form).
pub fn parse_hunks(v: Option<&Value>) -> Result<Vec<Hunk>, String> {
    let arr = v.and_then(Value::as_array).ok_or("`hunks` must be an array")?;
    arr.iter()
        .map(|h| {
            let o = h.as_object().ok_or("a hunk is an object {start, end, insert}")?;
            let n = |k: &str| o.get(k).and_then(Value::as_u64).map(|n| n as usize).ok_or(format!("a hunk's `{k}` must be a whole number"));
            let insert = o.get("insert").and_then(Value::as_str).ok_or("a hunk's `insert` must be a string")?;
            Ok(Hunk { start: n("start")?, end: n("end")?, insert: insert.to_string() })
        })
        .collect()
}

/// The `op` of a frame, when it is readable (for refusing a frame K2
/// can't take, e.g. one over [`FRAME_CAP`]).
pub fn op_of(v: &Value) -> Option<u64> {
    v.get("op").and_then(Value::as_u64)
}

pub fn parse(text: &str) -> Result<ClientFrame, BadFrame> {
    let v: Value = serde_json::from_str(text).map_err(|e| BadFrame { op: None, message: format!("not JSON: {e}") })?;
    let op = op_of(&v);
    let bad = |m: String| BadFrame { op, message: m };
    let o = v.as_object().ok_or_else(|| bad("a frame is a JSON object".into()))?;
    // The Thread's viewing frame (G8, the addendum).
    if o.get("t").and_then(Value::as_str) == Some("viewing") {
        return o.get("visible").and_then(Value::as_bool).map(ClientFrame::Visible).ok_or_else(|| bad("`visible` must be a boolean".into()));
    }
    let ty = o.get("type").and_then(Value::as_str).ok_or_else(|| bad("a frame needs a `type`".into()))?;
    let f = match ty {
        "edit" => ClientFrame::Edit {
            op: u64_field(o, "op").map_err(bad)?,
            root: str_field(o, "root").map_err(bad)?,
            base: u64_field(o, "base").map_err(bad)?,
            hunks: parse_hunks(o.get("hunks")).map_err(bad)?,
            group: match o.get("group") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(s.clone()),
                Some(_) => return Err(bad("`group` must be a string".into())),
            },
        },
        "since" => ClientFrame::Since { op: u64_field(o, "op").map_err(bad)?, version: u64_field(o, "version").map_err(bad)? },
        "cursor" => ClientFrame::Cursor {
            root: str_field(o, "root").map_err(bad)?,
            version: u64_field(o, "version").map_err(bad)?,
            anchor: u64_field(o, "anchor").map_err(bad)? as usize,
            head: u64_field(o, "head").map_err(bad)? as usize,
        },
        "undo" => ClientFrame::Undo { op: u64_field(o, "op").map_err(bad)? },
        "redo" => ClientFrame::Redo { op: u64_field(o, "op").map_err(bad)? },
        "snapshot" => ClientFrame::Snapshot { op: u64_field(o, "op").map_err(bad)? },
        "visible" => ClientFrame::Visible(o.get("visible").and_then(Value::as_bool).ok_or_else(|| bad("`visible` must be a boolean".into()))?),
        "view" => ClientFrame::View {
            root: o.get("root").and_then(Value::as_str).map(str::to_string),
            first: u64_field(o, "first").map_err(bad)? as usize,
            last: u64_field(o, "last").map_err(bad)? as usize,
        },
        other => return Err(bad(format!("unknown frame type `{other}`"))),
    };
    Ok(f)
}

/// The reply to a frame K2 couldn't take: `refused {op, error}` when it has
/// an op, else `error {error}` (G2). The error is the kit's `MPC-E113`.
pub fn bad_frame_reply(b: &BadFrame) -> Value {
    let error = bad_edit(b.message.clone());
    match b.op {
        Some(op) => json!({ "type": "refused", "op": op, "error": error }),
        None => json!({ "type": "error", "error": error }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_client_frame_parses_and_junk_is_refused_with_its_op() {
        let e = parse(r#"{"type":"edit","op":3,"root":"source","base":7,"hunks":[{"start":1,"end":2,"insert":"é"}],"group":"g1"}"#)
            .expect("edit");
        assert_eq!(
            e,
            ClientFrame::Edit {
                op: 3,
                root: "source".into(),
                base: 7,
                hunks: vec![Hunk { start: 1, end: 2, insert: "é".into() }],
                group: Some("g1".into())
            }
        );
        assert_eq!(parse(r#"{"type":"since","op":4,"version":9}"#), Ok(ClientFrame::Since { op: 4, version: 9 }));
        assert_eq!(
            parse(r#"{"type":"cursor","root":"source","version":9,"anchor":1,"head":4}"#),
            Ok(ClientFrame::Cursor { root: "source".into(), version: 9, anchor: 1, head: 4 })
        );
        assert_eq!(parse(r#"{"type":"undo","op":5}"#), Ok(ClientFrame::Undo { op: 5 }));
        assert_eq!(parse(r#"{"type":"redo","op":6}"#), Ok(ClientFrame::Redo { op: 6 }));
        assert_eq!(parse(r#"{"type":"snapshot","op":7}"#), Ok(ClientFrame::Snapshot { op: 7 }));
        assert_eq!(parse(r#"{"t":"viewing","visible":false}"#), Ok(ClientFrame::Visible(false)));
        assert_eq!(parse(r#"{"type":"visible","visible":true}"#), Ok(ClientFrame::Visible(true)));
        assert_eq!(parse(r#"{"type":"view","first":2,"last":9}"#), Ok(ClientFrame::View { root: None, first: 2, last: 9 }));

        let b = parse(r#"{"type":"edit","op":8,"root":"source","base":1,"hunks":[[0,0,"x"]]}"#).expect_err("array hunk");
        assert_eq!(b.op, Some(8), "refused with its op");
        let r = bad_frame_reply(&b);
        assert_eq!(r["type"], "refused");
        assert_eq!(r["op"], 8);
        assert_eq!(r["error"]["id"], "MPC-E113", "{r}");
        let b = parse("{nope").expect_err("not json");
        assert_eq!(b.op, None);
        assert_eq!(bad_frame_reply(&b)["type"], "error");
        let b = parse(r#"{"type":"teleport","op":1}"#).expect_err("unknown type");
        assert!(b.message.contains("teleport"), "{}", b.message);
        assert!(parse(r#"{"type":"edit","op":-1,"root":"source","base":1,"hunks":[]}"#).is_err(), "negative op");
    }

    #[test]
    fn close_reasons_say_whether_to_retry() {
        for (r, s, retry) in [
            (CloseReason::Left, "left", false),
            (CloseReason::Idle, "idle", true),
            (CloseReason::Kicked, "kicked", false),
            (CloseReason::RightsLost, "rights_lost", false),
            (CloseReason::Deleted, "deleted", false),
            (CloseReason::Moved, "moved", true),
            (CloseReason::Released, "released", true),
            (CloseReason::CutOff, "cut_off", true),
            (CloseReason::Declined, "declined", false),
            (CloseReason::NotFound, "not_found", false),
            (CloseReason::Auth, "auth", false),
            (CloseReason::Network, "network", true),
            (CloseReason::Closed, "closed", false),
        ] {
            assert_eq!(r.as_str(), s);
            assert_eq!(r.retry(), retry, "{s}");
        }
        let mut c = Closed::k2(CloseReason::Closed, "core_off", "the Markdown core is off");
        c.core = Some("markdown".into());
        c.version = Some(12);
        c.written = Some(true);
        assert_eq!(
            c.json(),
            json!({"type":"closed","reason":"closed","retry":false,"code":"core_off","message":"the Markdown core is off","version":12,"written":true,"core":"markdown"})
        );
    }

    #[test]
    fn a_kit_refusal_is_the_kits_error_report() {
        let e = error_json(&SessionError::RateLimited { retry_after_ms: 50 }.report());
        assert_eq!(e["id"], "MPC-E901", "{e}");
        assert!(e["message"].as_str().is_some_and(|m| !m.is_empty()), "{e}");
    }
}
