//! The Live socket's frames (`WS /cli/docs/live`, [`super::live`]), as the
//! kit's `client/WIRE.md` v2 (kit main `bea1885`) specifies them, plus K2's
//! own extra fields (marked below).
//!
//! **Encoded with the kit's own JSON writer** (`k2_mpc_base::errors::Value`,
//! as `ErrorReport::to_json`): it keeps object keys in insertion order, so
//! every value here is byte for byte what the kit's golden files hold
//! (`client/test/golden/*.json`, written by the kit's
//! `crates/base/tests/wire_golden.rs`; this module's tests replay that
//! scenario and compare). The kit's Rust types carry no serde.
//!
//! Client frames are read with serde (they are small and their key order
//! doesn't matter).

use serde_json::Value as Json;

use k2_mpc_base::errors::{lookup, ErrorReport, Value};
use k2_mpc_base::session::{
    ChangeEvent, CursorAt, LeaveReason, LineRange, Overwrite, ParticipantKind, Rights, Role, RosterEvent,
    RosterMember, SessionError, UndoOutcome, Who,
};
use k2_mpc_base::store::Seq;
use k2_mpc_base::text::Hunk;

/// The frame cap (WIRE.md "Frame cap", `FRAME_MAX`): 4,194,304 bytes of
/// UTF-8 JSON, both directions.
pub const FRAME_MAX: usize = 4 << 20;

/// The WebSocket layer's own limit (message and frame). A text frame over
/// [`FRAME_MAX`] and up to this is refused in order and the socket stays;
/// past it the read fails and the socket closes (`cut_off`, 1009). K2's,
/// not the wire's: it bounds what one socket can make the daemon buffer.
pub const HARD_CAP: usize = 8 << 20;

/// Why a Live socket ended: WIRE.md "Closing", 13 rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    Left,
    Idle,
    Kicked,
    RightsLost,
    Deleted,
    Moved,
    Released,
    CutOff,
    Declined,
    NotFound,
    Auth,
    /// The transport's own (a dropped socket); never in a frame.
    Network,
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

    /// The table's retry column (the client's `canRetry`): only `cut_off`
    /// and `network`.
    pub fn retry(self) -> bool {
        matches!(self, CloseReason::CutOff | CloseReason::Network)
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
    /// The daemon is handing off or stopping, or live docs are still
    /// starting: no `closed` frame (the transport's `network`: reconnect).
    pub const RESTART: u16 = 1012;
    /// The core was switched off (PRD multiplayer-cores-settings §6.1).
    pub const CORE_OFF: u16 = 4010;
}

/// The last frame before a close: `closed {reason, error?}`, and K2's
/// extras after them (`retry`, `code`, `message`, `version`, `written`,
/// `core`, `path`).
#[derive(Debug, Clone, PartialEq)]
pub struct Closed {
    pub reason: CloseReason,
    /// The kit's report (`MPC-…`), when there is one.
    pub error: Option<ErrorReport>,
    /// K2's own code (`core_off`, `secret`, `not_a_text_doc`...).
    pub code: Option<String>,
    pub message: Option<String>,
    /// On `core_off`: the version written back, and whether it was.
    pub version: Option<Seq>,
    pub written: Option<bool>,
    pub core: Option<String>,
    /// On `moved`: the doc's new workspace path.
    pub path: Option<String>,
    /// The WebSocket close code that follows.
    pub ws_code: u16,
}

impl Closed {
    pub fn new(reason: CloseReason) -> Self {
        Closed {
            reason,
            error: None,
            code: None,
            message: None,
            version: None,
            written: None,
            core: None,
            path: None,
            ws_code: code::NORMAL,
        }
    }

    /// K2's own refusal: a code and a message.
    pub fn k2(reason: CloseReason, code: &str, message: impl Into<String>) -> Self {
        Closed { code: Some(code.to_string()), message: Some(message.into()), ..Closed::new(reason) }
    }

    /// The kit's refusal.
    pub fn kit(reason: CloseReason, e: &SessionError) -> Self {
        Closed { error: Some(e.report()), ..Closed::new(reason) }
    }

    pub fn value(&self) -> Value {
        let mut o: Vec<(String, Value)> = vec![("type".into(), "closed".into()), ("reason".into(), self.reason.as_str().into())];
        if let Some(e) = &self.error {
            o.push(("error".into(), e.to_value()));
        }
        o.push(("retry".into(), self.reason.retry().into()));
        let mut opt = |k: &str, v: Option<Value>| {
            if let Some(v) = v {
                o.push((k.to_string(), v));
            }
        };
        opt("code", self.code.as_deref().map(Value::from));
        opt("message", self.message.as_deref().map(Value::from));
        opt("version", self.version.map(Value::from));
        opt("written", self.written.map(Value::from));
        opt("core", self.core.as_deref().map(Value::from));
        opt("path", self.path.as_deref().map(Value::from));
        Value::Obj(o)
    }
}

/// A kit report by id (`lookup`), for the reports the host builds itself.
pub fn report(id: &str, message: impl Into<String>) -> ErrorReport {
    ErrorReport::new(lookup(id).unwrap_or_else(|| panic!("the vendored kit has no {id}")), message)
}

/// `MPC-E113` for a client frame over the cap (WIRE.md "Frame cap").
pub fn over_cap(bytes: usize) -> ErrorReport {
    report("MPC-E113", format!("bad edit: the frame is {bytes} bytes, over the {FRAME_MAX}-byte cap"))
        .with("bytes", bytes)
        .with("frame_max", FRAME_MAX)
}

/// `MPC-E113` for a client frame K2 couldn't read.
pub fn unreadable(why: &str) -> ErrorReport {
    report("MPC-E113", format!("bad edit: {why}"))
}

/// `MPC-E112` for a `since` reply that doesn't fit one frame.
pub fn events_over_cap(bytes: usize) -> ErrorReport {
    report("MPC-E112", format!("the changes since don't fit one frame ({bytes} bytes, over the {FRAME_MAX}-byte cap); reload"))
        .with("limit", "frame")
        .with("bytes", bytes)
        .with("frame_max", FRAME_MAX)
}

/// `closed {declined}` with `MPC-E401` for a `welcome` or `snapshot` that
/// doesn't fit one frame: no retry.
pub fn too_large(bytes: usize) -> Closed {
    let e = report("MPC-E401", "live editing is off: the text doesn't fit one frame (too_large)")
        .with("limit", "frame")
        .with("bytes", bytes)
        .with("frame_max", FRAME_MAX);
    Closed { error: Some(e), code: Some("too_large".into()), ..Closed::new(CloseReason::Declined) }
}

pub fn hunk(h: &Hunk) -> Value {
    Value::obj([("start", Value::from(h.start)), ("end", Value::from(h.end)), ("insert", Value::from(h.insert.as_str()))])
}

pub fn hunks(hs: &[Hunk]) -> Value {
    Value::List(hs.iter().map(hunk).collect())
}

/// `ChangeEvent`: `{version, pid, client, via, root, hunks, at_ms, on_behalf_of}`
/// (`on_behalf_of` is `null` when absent, never left out).
pub fn change(e: &ChangeEvent) -> Value {
    Value::obj([
        ("version", Value::from(e.version)),
        ("pid", Value::from(e.pid.as_str())),
        ("client", Value::from(e.client)),
        ("via", Value::from(e.via.as_str())),
        ("root", Value::from(e.root.as_str())),
        ("hunks", hunks(&e.hunks)),
        ("at_ms", Value::from(e.at_ms)),
        ("on_behalf_of", e.on_behalf_of.as_deref().map_or(Value::Null, Value::from)),
    ])
}

/// `"edit"` (read and edit), `"read"` (read only), else `"none"`.
pub fn rights(r: Rights) -> &'static str {
    if r.read && r.edit {
        "edit"
    } else if r.read {
        "read"
    } else {
        "none"
    }
}

fn role(r: Role) -> &'static str {
    match r {
        Role::Editor => "editor",
        Role::Viewer => "viewer",
    }
}

fn kind(k: ParticipantKind) -> &'static str {
    match k {
        ParticipantKind::Person => "person",
        ParticipantKind::Agent => "agent",
        ParticipantKind::App => "app",
        ParticipantKind::External => "external",
        ParticipantKind::Restore => "restore",
        ParticipantKind::Import => "import",
    }
}

fn who(w: &Who) -> Value {
    Value::from(w.to_string())
}

fn lines(l: &LineRange) -> Value {
    Value::obj([("root", Value::from(l.root.as_str())), ("first", Value::from(l.first)), ("last", Value::from(l.last))])
}

/// A caret: `{root, anchor, head, version, anchor_lc, head_lc}`.
fn caret(c: &CursorAt, version: Seq) -> Value {
    Value::obj([
        ("root", Value::from(c.cursor.root.as_str())),
        ("anchor", Value::from(c.cursor.anchor)),
        ("head", Value::from(c.cursor.head)),
        ("version", Value::from(version)),
        ("anchor_lc", Value::from(format!("{}:{}", c.anchor.line, c.anchor.col))),
        ("head_lc", Value::from(format!("{}:{}", c.head.line, c.head.col))),
    ])
}

/// `RosterMember` → the wire's entry `{handle, name, colour, kind, role, away, cursor, rights}`.
fn member(m: &RosterMember, version: Seq) -> Value {
    let p = &m.entry.participant;
    Value::obj([
        ("handle", who(&m.who)),
        ("name", Value::from(p.name.as_str())),
        ("colour", Value::from(p.colour.as_str())),
        ("kind", Value::from(kind(p.kind))),
        ("role", Value::from(role(m.entry.role))),
        ("away", Value::from(m.entry.away)),
        ("cursor", m.entry.cursor.as_ref().map_or(Value::Null, |c| caret(c, version))),
        ("rights", Value::from(rights(m.rights))),
    ])
}

/// One `overwritten` event (WIRE.md "The `overwritten` event", kit
/// v0.9.7.2): an outside save may have removed `to`'s text. It holds their
/// own text (`cells[].yours`, `message`): only ever sent to them, and never
/// logged.
pub fn overwritten(to: Value, o: &Overwrite) -> Value {
    let cell = |region: &str, col: Option<usize>, yours: &str, file: &str, cut: bool| {
        Value::obj([
            ("region", Value::from(region)),
            ("col", col.map_or(Value::Null, Value::from)),
            ("yours", Value::from(yours)),
            ("file", Value::from(file)),
            ("cut", Value::from(cut)),
        ])
    };
    let mut cells = Vec::with_capacity(1 + o.also.len());
    if !o.is_rollup() {
        cells.push(cell(&o.region, o.col, &o.yours, &o.file, o.cut));
    }
    cells.extend(o.also.iter().map(|c| cell(&c.region, c.col, &c.yours, &c.file, c.cut)));
    Value::obj([
        ("kind", Value::from("overwritten")),
        ("to", to),
        ("change", Value::from(o.change)),
        ("version", o.version.map_or(Value::Null, Value::from)),
        ("restore_from", o.restore_from.map_or(Value::Null, Value::from)),
        ("source", Value::from(o.source)),
        ("rule", Value::from(o.rule)),
        ("row", o.row.as_deref().map_or(Value::Null, Value::from)),
        ("line", o.line.map_or(Value::Null, Value::from)),
        ("cells", Value::List(cells)),
        ("more", Value::from(o.more)),
        ("not_shown", Value::from(o.not_shown)),
        ("withheld", Value::from(o.withheld)),
        ("no_history", Value::from(o.no_history)),
        ("message", Value::from(o.to_string())),
    ])
}

/// One `RosterEvent`, tagged by `kind`. `None` for an event this K2 doesn't
/// know (the kit's enum is non-exhaustive): it isn't sent.
pub fn roster_event(e: &RosterEvent) -> Option<Value> {
    roster_event_for(e, Value::Null)
}

/// [`roster_event`] for the subscriber `me` (its `who`): a snapshot's own
/// waiting `overwritten` notes name it as `to`.
pub fn roster_event_for(e: &RosterEvent, me: Value) -> Option<Value> {
    Some(match e {
        RosterEvent::Snapshot { version, members, watching, overwritten: waiting, .. } => {
            let mut o = vec![
                ("kind".to_string(), Value::from("snapshot")),
                ("version".to_string(), Value::from(*version)),
                ("members".to_string(), Value::List(members.iter().map(|m| member(m, *version)).collect())),
                ("watching".to_string(), Value::from(*watching)),
            ];
            // Only when there are some (WIRE.md: optional), so a snapshot
            // without notes is byte for byte the kit's golden one.
            if !waiting.is_empty() {
                o.push((
                    "overwritten".to_string(),
                    Value::List(waiting.iter().map(|w| overwritten(me.clone(), w)).collect()),
                ));
            }
            Value::Obj(o)
        }
        RosterEvent::Overwritten { to, overwrite, .. } => overwritten(who(to), overwrite),
        // The host closed the session: the last event (a Live socket ends
        // with its own `closed` frame instead, and never sends this one).
        RosterEvent::Closed { reason, .. } => {
            Value::obj([("kind", Value::from("closed")), ("reason", Value::from(*reason))])
        }
        RosterEvent::Joined { who: w, participant: p, rights: r, .. } => Value::obj([
            ("kind", Value::from("joined")),
            ("who", who(w)),
            (
                "participant",
                Value::obj([
                    ("name", Value::from(p.name.as_str())),
                    ("colour", Value::from(p.colour.as_str())),
                    ("kind", Value::from(kind(p.kind))),
                    ("via", Value::from(p.via.as_str())),
                ]),
            ),
            ("rights", Value::from(rights(*r))),
        ]),
        RosterEvent::Left { who: w, reason: r, .. } => Value::obj([
            ("kind", Value::from("left")),
            ("who", who(w)),
            ("reason", Value::from(CloseReason::from_leave(*r).as_str())),
        ]),
        RosterEvent::Rights { who: w, rights: r, .. } => {
            Value::obj([("kind", Value::from("rights")), ("who", who(w)), ("rights", Value::from(rights(*r)))])
        }
        RosterEvent::Role { who: w, role: r, .. } => {
            Value::obj([("kind", Value::from("role")), ("who", who(w)), ("role", Value::from(role(*r)))])
        }
        RosterEvent::Away { who: w, away, .. } => {
            Value::obj([("kind", Value::from("away")), ("who", who(w)), ("away", Value::from(*away))])
        }
        RosterEvent::Cursor { who: w, cursor, version, .. } => Value::obj([
            ("kind", Value::from("cursor")),
            ("who", who(w)),
            ("cursor", cursor.as_ref().map_or(Value::Null, |c| caret(c, *version))),
            ("version", Value::from(*version)),
        ]),
        RosterEvent::ClaimStarted { who: w, claim, lines: l, note, hold, for_edit, expires_ms, version, .. } => {
            Value::obj([
                ("kind", Value::from("claim_started")),
                ("who", who(w)),
                ("claim", Value::from(claim.to_string())),
                ("lines", lines(l)),
                ("note", note.as_deref().map_or(Value::Null, Value::from)),
                ("hold", Value::from(*hold)),
                ("for_edit", Value::from(*for_edit)),
                ("expires_ms", Value::from(*expires_ms)),
                ("version", Value::from(*version)),
            ])
        }
        RosterEvent::ClaimEnded { who: w, claim, how, lines: l, version, .. } => Value::obj([
            ("kind", Value::from("claim_ended")),
            ("who", who(w)),
            ("claim", Value::from(claim.to_string())),
            ("how", Value::from(how.as_str())),
            ("lines", lines(l)),
            ("version", Value::from(*version)),
        ]),
        RosterEvent::Note { to, from, kind: k, lines: l, claim, .. } => Value::obj([
            ("kind", Value::from("note")),
            ("to", who(to)),
            ("from", who(from)),
            ("note", Value::from(k.as_str())),
            ("lines", lines(l)),
            ("claim", claim.map_or(Value::Null, |c| Value::from(c.to_string()))),
        ]),
        RosterEvent::Watching { count, .. } => Value::obj([("kind", Value::from("watching")), ("count", Value::from(*count))]),
        _ => return None,
    })
}

// ── Frames ───────────────────────────────────────────────────────────

pub fn change_frame(e: &ChangeEvent) -> Value {
    Value::obj([("type", Value::from("change")), ("event", change(e))])
}

pub fn ack_frame(op: u64, version: Seq, hs: &[Hunk]) -> Value {
    Value::obj([("type", Value::from("ack")), ("op", Value::from(op)), ("version", Value::from(version)), ("hunks", hunks(hs))])
}

pub fn refused_frame(op: u64, e: &ErrorReport) -> Value {
    Value::obj([("type", Value::from("refused")), ("op", Value::from(op)), ("error", e.to_value())])
}

pub fn error_frame(e: &ErrorReport) -> Value {
    Value::obj([("type", Value::from("error")), ("error", e.to_value())])
}

pub fn events_frame(op: u64, evs: &[ChangeEvent]) -> Value {
    Value::obj([("type", Value::from("events")), ("op", Value::from(op)), ("events", Value::List(evs.iter().map(change).collect()))])
}

pub fn snapshot_frame(op: u64, text: &str, version: Seq) -> Value {
    Value::obj([("type", Value::from("snapshot")), ("op", Value::from(op)), ("text", Value::from(text)), ("version", Value::from(version))])
}

pub fn roster_frame(event: Value) -> Value {
    Value::obj([("type", Value::from("roster_event")), ("event", event)])
}

/// The reply to `undo` / `redo` (WIRE.md "Undo and redo"): `undone {op,
/// steps, version, notice?}` (`steps: 0`: nothing to undo; `notice`:
/// `MPC-E206` / `MPC-E207`), or `refused` with `MPC-E201` on a conflict.
pub fn undo_reply(op: u64, out: &UndoOutcome, version_now: Seq) -> Value {
    match out {
        UndoOutcome::Applied { steps, result, .. } => undone_frame(op, *steps, result.version, None),
        UndoOutcome::Nothing => undone_frame(op, 0, version_now, None),
        UndoOutcome::HistoryLost | UndoOutcome::ClearedByPurge => undone_frame(op, 0, version_now, out.refusal()),
        other => match other.refusal() {
            Some(e) => refused_frame(op, &e),
            None => undone_frame(op, 0, version_now, None),
        },
    }
}

fn undone_frame(op: u64, steps: usize, version: Seq, notice: Option<ErrorReport>) -> Value {
    let mut o = vec![
        ("type".to_string(), Value::from("undone")),
        ("op".to_string(), Value::from(op)),
        ("steps".to_string(), Value::from(steps)),
        ("version".to_string(), Value::from(version)),
    ];
    if let Some(n) = notice {
        o.push(("notice".to_string(), n.to_value()));
    }
    Value::Obj(o)
}

/// The first frame: `welcome {root, version, text?, client, self, rights}`,
/// then K2's extras (`core`, `family`, `path`).
#[allow(clippy::too_many_arguments)]
pub fn welcome_frame(
    root: &str,
    version: Seq,
    text: Option<&str>,
    client: u64,
    self_who: &str,
    rights_word: &str,
    core: &str,
    family: &str,
    path: &str,
) -> Value {
    let mut o = vec![
        ("type".to_string(), Value::from("welcome")),
        ("root".to_string(), Value::from(root)),
        ("version".to_string(), Value::from(version)),
    ];
    if let Some(t) = text {
        o.push(("text".to_string(), Value::from(t)));
    }
    for (k, v) in [
        ("client", Value::from(client)),
        ("self", Value::from(self_who)),
        ("rights", Value::from(rights_word)),
        ("core", Value::from(core)),
        ("family", Value::from(family)),
        ("path", Value::from(path)),
    ] {
        o.push((k.to_string(), v));
    }
    Value::Obj(o)
}

// ── Client frames ────────────────────────────────────────────────────

/// A frame from the client (WIRE.md "Client → host").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientFrame {
    Edit { op: u64, root: String, base: u64, hunks: Vec<Hunk>, group: Option<String> },
    Since { op: u64, version: u64 },
    Cursor { root: String, version: u64, anchor: usize, head: usize },
    Undo { op: u64 },
    Redo { op: u64 },
    Snapshot { op: u64 },
    /// `visible {visible}` (also the Thread's `{"t":"viewing","visible"}`).
    Visible(bool),
    View { root: Option<String>, first: usize, last: usize },
    /// `hello {root, resume?}`: K2 takes both in the connect URL, so it is
    /// ignored (the URL's win).
    Hello,
}

/// Why a client frame was refused: its `op` when it had one (the reply is
/// then `refused`, else `error`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadFrame {
    pub op: Option<u64>,
    pub message: String,
}

impl BadFrame {
    /// `refused {op, error}` with an op, else `error {error}`: `MPC-E113`.
    pub fn reply(&self) -> Value {
        let e = unreadable(&self.message);
        match self.op {
            Some(op) => refused_frame(op, &e),
            None => error_frame(&e),
        }
    }
}

fn u64_field(o: &serde_json::Map<String, Json>, k: &str) -> Result<u64, String> {
    o.get(k).and_then(Json::as_u64).ok_or_else(|| format!("`{k}` must be a whole number"))
}

fn str_field(o: &serde_json::Map<String, Json>, k: &str) -> Result<String, String> {
    o.get(k).and_then(Json::as_str).map(str::to_string).ok_or_else(|| format!("`{k}` must be a string"))
}

/// Hunks as objects `{start, end, insert}`.
pub fn parse_hunks(v: Option<&Json>) -> Result<Vec<Hunk>, String> {
    let arr = v.and_then(Json::as_array).ok_or("`hunks` must be an array")?;
    arr.iter()
        .map(|h| {
            let o = h.as_object().ok_or("a hunk is an object {start, end, insert}")?;
            let n = |k: &str| o.get(k).and_then(Json::as_u64).map(|n| n as usize).ok_or(format!("a hunk's `{k}` must be a whole number"));
            let insert = o.get("insert").and_then(Json::as_str).ok_or("a hunk's `insert` must be a string")?;
            Ok(Hunk { start: n("start")?, end: n("end")?, insert: insert.to_string() })
        })
        .collect()
}

/// A frame's `type` and `op`, when readable (for a frame over the cap).
pub fn type_and_op(text: &str) -> (Option<String>, Option<u64>) {
    match serde_json::from_str::<Json>(text) {
        Ok(v) => (v.get("type").and_then(Json::as_str).map(str::to_string), v.get("op").and_then(Json::as_u64)),
        Err(_) => (None, None),
    }
}

pub fn parse(text: &str) -> Result<ClientFrame, BadFrame> {
    let v: Json = serde_json::from_str(text).map_err(|e| BadFrame { op: None, message: format!("the frame is not JSON ({e})") })?;
    let op = v.get("op").and_then(Json::as_u64);
    let bad = |m: String| BadFrame { op, message: m };
    let o = v.as_object().ok_or_else(|| bad("a frame is a JSON object".into()))?;
    if o.get("t").and_then(Json::as_str) == Some("viewing") {
        return o.get("visible").and_then(Json::as_bool).map(ClientFrame::Visible).ok_or_else(|| bad("`visible` must be a boolean".into()));
    }
    let ty = o.get("type").and_then(Json::as_str).ok_or_else(|| bad("a frame needs a `type`".into()))?;
    Ok(match ty {
        "edit" => ClientFrame::Edit {
            op: u64_field(o, "op").map_err(bad)?,
            root: str_field(o, "root").map_err(bad)?,
            base: u64_field(o, "base").map_err(bad)?,
            hunks: parse_hunks(o.get("hunks")).map_err(bad)?,
            group: match o.get("group") {
                None | Some(Json::Null) => None,
                Some(Json::String(s)) => Some(s.clone()),
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
        "visible" => ClientFrame::Visible(o.get("visible").and_then(Json::as_bool).ok_or_else(|| bad("`visible` must be a boolean".into()))?),
        "view" => ClientFrame::View {
            root: o.get("root").and_then(Json::as_str).map(str::to_string),
            first: u64_field(o, "first").map_err(bad)? as usize,
            last: u64_field(o, "last").map_err(bad)? as usize,
        },
        "hello" => ClientFrame::Hello,
        other => return Err(bad(format!("unknown frame type `{other}`"))),
    })
}

#[cfg(test)]
#[path = "live_wire_golden_tests.rs"]
mod golden_tests;

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
        assert_eq!(parse(r#"{"type":"hello","root":"source","resume":4}"#), Ok(ClientFrame::Hello));

        let b = parse(r#"{"type":"edit","op":8,"root":"source","base":1,"hunks":[[0,0,"x"]]}"#).expect_err("array hunk");
        assert_eq!(b.op, Some(8), "refused with its op");
        let r = b.reply().to_json();
        assert!(r.starts_with(r#"{"type":"refused","op":8,"error":{"id":"MPC-E113""#), "{r}");
        let b = parse("{nope").expect_err("not json");
        assert_eq!(b.op, None);
        assert!(b.reply().to_json().starts_with(r#"{"type":"error","error":{"id":"MPC-E113""#));
        let b = parse(r#"{"type":"teleport","op":1}"#).expect_err("unknown type");
        assert!(b.message.contains("teleport"), "{}", b.message);
        assert!(parse(r#"{"type":"edit","op":-1,"root":"source","base":1,"hunks":[]}"#).is_err(), "negative op");
        assert_eq!(type_and_op(r#"{"type":"edit","op":12}"#), (Some("edit".into()), Some(12)));
    }

    #[test]
    fn close_reasons_follow_the_wire_table() {
        for (r, s, retry) in [
            (CloseReason::Left, "left", false),
            (CloseReason::Idle, "idle", false),
            (CloseReason::Kicked, "kicked", false),
            (CloseReason::RightsLost, "rights_lost", false),
            (CloseReason::Deleted, "deleted", false),
            (CloseReason::Moved, "moved", false),
            (CloseReason::Released, "released", false),
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
        let mut c = Closed::k2(CloseReason::Closed, "core_off", "the markdown core is off");
        c.core = Some("markdown".into());
        c.version = Some(12);
        c.written = Some(true);
        assert_eq!(
            c.value().to_json(),
            r#"{"type":"closed","reason":"closed","retry":false,"code":"core_off","message":"the markdown core is off","version":12,"written":true,"core":"markdown"}"#
        );
    }

    #[test]
    fn the_hosts_own_reports_match_the_kits_golden_ones() {
        // errors.json `bad_edit_frame_cap` and `declined_too_large`, as the kit wrote them.
        assert_eq!(
            over_cap(4_194_400).to_json(),
            report("MPC-E113", "bad edit: the frame is 4194400 bytes, over the 4194304-byte cap")
                .with("bytes", 4_194_400u64)
                .with("frame_max", 4_194_304u64)
                .to_json()
        );
        let c = too_large(6_291_502);
        assert_eq!(c.reason, CloseReason::Declined);
        assert!(!c.reason.retry());
        let e = c.error.expect("E401");
        assert_eq!(e.id(), "MPC-E401");
        assert!(e.to_json().contains(r#""details":{"limit":"frame","bytes":6291502,"frame_max":4194304}"#), "{}", e.to_json());
        let e = events_over_cap(5 << 20);
        assert_eq!(e.id(), "MPC-E112");
        assert!(e.to_json().contains(r#""limit":"frame""#));
    }

    #[test]
    fn undo_replies_are_undone_or_refused_e201() {
        assert_eq!(undo_reply(3, &UndoOutcome::Nothing, 9).to_json(), r#"{"type":"undone","op":3,"steps":0,"version":9}"#);
        let lost = undo_reply(4, &UndoOutcome::HistoryLost, 9).to_json();
        assert!(lost.starts_with(r#"{"type":"undone","op":4,"steps":0,"version":9,"notice":{"id":"MPC-E206""#), "{lost}");
        let purged = undo_reply(5, &UndoOutcome::ClearedByPurge, 9).to_json();
        assert!(purged.contains(r#""notice":{"id":"MPC-E207""#), "{purged}");
        let c = undo_reply(6, &UndoOutcome::Conflicts { step: 1, conflicts: vec![] }, 9).to_json();
        assert!(c.starts_with(r#"{"type":"refused","op":6,"error":{"id":"MPC-E201""#), "{c}");
    }
}
