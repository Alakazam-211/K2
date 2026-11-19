//! The Live socket's frames (`WS /cli/docs/live`, [`super::live`]), as the
//! kit's `client/WIRE.md` **wire 3** (kit v0.15.x, `session::WIRE_VERSION`)
//! specifies them, plus K2's own extra fields (marked below). Wire 3 only
//! adds to wire 2: a change's `turn` and `verb`; the claim events' `actor`
//! and times; element presence (`selection`, `pointer`, `viewport`,
//! `focus`, `range`) as roster events and entry fields; `elem_claims`;
//! `welcome.wire`. A wire-2 client ignores what it doesn't know.
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
    ChangeEvent, ClaimActor, ClaimSpan, CursorAt, ElemFocus, ElemRange, ElemSelection, LeaveReason, LineRange, Overwrite,
    ParticipantKind, PlayState, Point, Rect, Rights, Role, RosterEvent, RosterMember, SessionError, TimePosition, UndoOutcome,
    Who,
};

/// The wire this host speaks (`welcome.wire`): the vendored kit's.
pub const WIRE: u32 = k2_mpc_base::session::WIRE_VERSION;
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

/// Why a Live socket ended: WIRE.md "Closing", 14 rows.
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
    /// Kit v0.23.0's `rolled_back`: grouped commits, a commit failed at the
    /// disk, and the doc reloaded at what was on disk; the client joins
    /// again (its `canRetry` retries it).
    RolledBack,
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
            CloseReason::RolledBack => "rolled_back",
        }
    }

    /// The table's retry column (the client's `canRetry`): `cut_off`,
    /// `network` and, since kit v0.23.0, `rolled_back`.
    pub fn retry(self) -> bool {
        matches!(self, CloseReason::CutOff | CloseReason::Network | CloseReason::RolledBack)
    }

    /// From the kit's leave reason.
    pub fn from_leave(r: LeaveReason) -> CloseReason {
        match r {
            LeaveReason::Left => CloseReason::Left,
            LeaveReason::Idle => CloseReason::Idle,
            LeaveReason::Kicked => CloseReason::Kicked,
            LeaveReason::RightsLost => CloseReason::RightsLost,
            LeaveReason::RolledBack => CloseReason::RolledBack,
            // `LeaveReason` is `#[non_exhaustive]` since kit v0.23.0: a
            // later reason reads as a plain leave until K2 names it.
            _ => CloseReason::Left,
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

/// `ChangeEvent`: `{version, pid, client, via, kind, root, hunks, at_ms,
/// on_behalf_of, turn, verb}` (`on_behalf_of`, `turn` and `verb` are `null`
/// when absent, never left out). `kind` (`ChangeKind::as_str`) is what lets
/// a client tell a lost edit that landed late from an undo that landed late
/// under the same client ID. `turn` (wire 3) is the host turn of the agent
/// step that made it ([`super::turn`]); `verb` the pack verb's id.
/// `kept_pairs` (kit v0.21.0, additive): on an outside save's change whose
/// merge kept both sides, where (`[{live: {first, last}, file: {first,
/// last}}]`, 1-based inclusive lines in the root's text after the change);
/// left out when there are none, as the kit's WIRE.md and goldens.
pub fn change(e: &ChangeEvent) -> Value {
    let mut fields = vec![
        ("version", Value::from(e.version)),
        ("pid", Value::from(e.pid.as_str())),
        ("client", Value::from(e.client)),
        ("via", Value::from(e.via.as_str())),
        ("kind", Value::from(e.kind.as_str())),
        ("root", Value::from(e.root.as_str())),
        ("hunks", hunks(&e.hunks)),
        ("at_ms", Value::from(e.at_ms)),
        ("on_behalf_of", e.on_behalf_of.as_deref().map_or(Value::Null, Value::from)),
        ("turn", e.turn.as_ref().map_or(Value::Null, |t| Value::from(t.as_str()))),
        ("verb", e.verb.as_deref().map_or(Value::Null, Value::from)),
    ];
    if !e.kept_pairs.is_empty() {
        let pairs = e.kept_pairs.iter().map(|p| Value::obj([("live", line_pair(p.live)), ("file", line_pair(p.file))]));
        fields.push(("kept_pairs", Value::List(pairs.collect())));
    }
    Value::obj(fields)
}

/// Lines `first..=last` (1-based, inclusive): `{first, last}`.
fn line_pair((first, last): (usize, usize)) -> Value {
    Value::obj([("first", Value::from(first)), ("last", Value::from(last))])
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

pub(crate) fn kind(k: ParticipantKind) -> &'static str {
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

/// A claim event's `actor` (wire 3): `{id, name, kind, colour}`, `id` the
/// event's `who`.
fn actor(w: &Who, a: &ClaimActor) -> Value {
    Value::obj([
        ("id", who(w)),
        ("name", Value::from(a.name.as_str())),
        ("kind", Value::from(kind(a.kind))),
        ("colour", Value::from(a.colour.as_str())),
    ])
}

/// Elements as the wire names them: by address (kit v0.16.0, contract gap 2:
/// `@sld:256/sp:7` for a deck; a pack that maps no addresses gives its keys,
/// so the bytes are as before). The kit fills the addresses on every
/// selection, focus, range and element claim it hands out.
fn addresses_value(addresses: &[String]) -> Value {
    Value::List(addresses.iter().map(|a| Value::from(a.as_str())).collect())
}

/// A coordinate: a whole number as an integer (as the kit's goldens), else
/// a float; a non-finite one (never sent by a setter) as `null`.
fn coord(v: f64) -> Value {
    match k2_mpc_base::errors::Finite::new(v) {
        None => Value::Null,
        Some(_) if v.fract() == 0.0 && v.abs() < 9.0e15 => Value::Int(v as i64),
        Some(f) => Value::Float(f),
    }
}

fn selection(s: &ElemSelection) -> Value {
    Value::obj([("root", Value::from(s.root.as_str())), ("keys", addresses_value(&s.addresses))])
}

fn point(p: &Point) -> Value {
    Value::obj([("x", coord(p.x())), ("y", coord(p.y()))])
}

fn rect(r: &Rect) -> Value {
    Value::obj([("x", coord(r.x())), ("y", coord(r.y())), ("w", coord(r.w())), ("h", coord(r.h()))])
}

fn focus(f: &ElemFocus) -> Value {
    Value::obj([("root", Value::from(f.root.as_str())), ("key", Value::from(f.address.as_str()))])
}

fn range(r: &ElemRange) -> Value {
    Value::obj([
        ("root", Value::from(r.root.as_str())),
        ("key", Value::from(r.address.as_str())),
        ("field", Value::from(r.field.as_str())),
        ("anchor", Value::from(r.anchor)),
        ("head", Value::from(r.head)),
    ])
}

/// The element presence of an entry or a device, each left out when `None`.
fn elem_fields(
    fields: &mut Vec<(&'static str, Value)>,
    s: &Option<ElemSelection>,
    p: &Option<Point>,
    r: &Option<Rect>,
    f: &Option<ElemFocus>,
    g: &Option<ElemRange>,
) {
    if let Some(s) = s {
        fields.push(("selection", selection(s)));
    }
    if let Some(p) = p {
        fields.push(("pointer", point(p)));
    }
    if let Some(r) = r {
        fields.push(("viewport", rect(r)));
    }
    if let Some(f) = f {
        fields.push(("focus", focus(f)));
    }
    if let Some(g) = g {
        fields.push(("range", range(g)));
    }
}

/// A time position (WIRE.md "JSON encodings", kit v0.18.0): `{at, state,
/// at_ms}`, then `in` and `rate` only when it has them.
fn time_position(t: &TimePosition) -> Value {
    let mut fields = vec![("at", Value::from(t.at())), ("state", Value::from(t.state().as_str())), ("at_ms", Value::from(t.at_ms()))];
    if let Some(a) = t.in_address() {
        fields.push(("in", Value::from(a)));
    }
    if let Some(r) = t.rate_text() {
        fields.push(("rate", Value::from(r)));
    }
    Value::obj(fields)
}

/// An entry's or a device's `view_address` and `time_position` (kit
/// v0.18.0), each left out when `None`, after the element presence.
fn view_time_fields(fields: &mut Vec<(&'static str, Value)>, view: &Option<String>, time: &Option<TimePosition>) {
    if let Some(v) = view {
        fields.push(("view_address", Value::from(v.as_str())));
    }
    if let Some(t) = time {
        fields.push(("time_position", time_position(t)));
    }
}

/// A `selection`, `pointer`, `viewport`, `focus` or `range` event: `client`
/// only when the event names one.
fn elem_event(kind: &'static str, w: &Who, value: Value, client: &Option<u64>) -> Value {
    let mut fields = vec![("kind", Value::from(kind)), ("who", who(w)), (kind, value)];
    if let Some(c) = client {
        fields.push(("client", Value::from(*c)));
    }
    Value::obj(fields)
}

/// `RosterMember` → the wire's entry `{handle, name, colour, kind, role, away, cursor, rights}`,
/// then `devices` (kit v0.10.1) only when the kit gives some (a text doc): one
/// `{client, cursor, label?}` per connection of the participant, in join order,
/// `label` only when the host gave one. A client hides only its own
/// connection's caret (`welcome.client`) and shows the person's other tabs.
/// Then (wire 3, keyed docs) the element presence on the entry and each
/// device, and `elem_claims` when the member holds claims on elements.
fn member(m: &RosterMember, version: Seq) -> Value {
    let p = &m.entry.participant;
    let mut fields = vec![
        ("handle", who(&m.who)),
        ("name", Value::from(p.name.as_str())),
        ("colour", Value::from(p.colour.as_str())),
        ("kind", Value::from(kind(p.kind))),
        ("role", Value::from(role(m.entry.role))),
        ("away", Value::from(m.entry.away)),
        ("cursor", m.entry.cursor.as_ref().map_or(Value::Null, |c| caret(c, version))),
        ("rights", Value::from(rights(m.rights))),
    ];
    if !m.entry.devices.is_empty() {
        let devices = m.entry.devices.iter().map(|d| {
            let mut o = vec![
                ("client", Value::from(d.client)),
                ("cursor", d.cursor.as_ref().map_or(Value::Null, |c| caret(c, version))),
            ];
            if let Some(l) = &d.label {
                o.push(("label", Value::from(l.as_str())));
            }
            let mut more = Vec::new();
            elem_fields(&mut more, &d.selection, &d.pointer, &d.viewport, &d.focus, &d.range);
            view_time_fields(&mut more, &d.view_address, &d.time);
            o.extend(more);
            Value::obj(o)
        });
        fields.push(("devices", Value::List(devices.collect())));
    }
    let e = &m.entry;
    elem_fields(&mut fields, &e.selection, &e.pointer, &e.viewport, &e.focus, &e.range);
    view_time_fields(&mut fields, &e.view_address, &e.time);
    let elem_claims: Vec<Value> = e
        .claims
        .iter()
        .filter_map(|c| match &c.span {
            ClaimSpan::Elems { root, .. } => Some(Value::obj([
                ("claim", Value::from(c.id.to_string())),
                ("root", Value::from(root.as_str())),
                ("keys", addresses_value(&c.addresses)),
                ("note", c.note.as_deref().map_or(Value::Null, Value::from)),
                ("hold", Value::from(c.hold)),
                ("expires_ms", Value::from(c.expires_ms)),
                ("started_ms", Value::from(c.taken_ms)),
            ])),
            _ => None,
        })
        .collect();
    if !elem_claims.is_empty() {
        fields.push(("elem_claims", Value::List(elem_claims)));
    }
    Value::obj(fields)
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
        // `client` (kit v0.10.1): the connection whose caret this is; absent
        // on the participant's own caret, which the kit sends last in a flush
        // when the connections' events didn't end on it (old clients keep one
        // caret a person: the last event's).
        RosterEvent::Cursor { who: w, cursor, version, client, .. } => {
            let mut fields = vec![
                ("kind", Value::from("cursor")),
                ("who", who(w)),
                ("cursor", cursor.as_ref().map_or(Value::Null, |c| caret(c, *version))),
                ("version", Value::from(*version)),
            ];
            if let Some(c) = client {
                fields.push(("client", Value::from(*c)));
            }
            Value::obj(fields)
        }
        // `bytes` (kit v0.21.0, additive): the claim's span when it was
        // taken, in UTF-8 bytes of its root's text at `version`, half-open
        // (`{start, end}`); left out when it has none (a grid's line claim).
        RosterEvent::ClaimStarted {
            who: w, claim, lines: l, bytes, note, hold, for_edit, expires_ms, version, actor: a, started_ms, ..
        } => {
            let mut fields = vec![
                ("kind", Value::from("claim_started")),
                ("who", who(w)),
                ("claim", Value::from(claim.to_string())),
                ("lines", lines(l)),
            ];
            if let Some(b) = bytes {
                fields.push(("bytes", Value::obj([("start", Value::from(b.start)), ("end", Value::from(b.end))])));
            }
            fields.extend([
                ("note", note.as_deref().map_or(Value::Null, Value::from)),
                ("hold", Value::from(*hold)),
                ("for_edit", Value::from(*for_edit)),
                ("expires_ms", Value::from(*expires_ms)),
                ("version", Value::from(*version)),
                ("actor", actor(w, a)),
                ("started_ms", Value::from(*started_ms)),
            ]);
            Value::obj(fields)
        }
        // Wire 3: a claim on elements (a deck's shapes or slides).
        RosterEvent::ClaimStartedElems { who: w, claim, root, addresses, note, hold, expires_ms, version, actor: a, started_ms, .. } => {
            Value::obj([
                ("kind", Value::from("claim_started_elems")),
                ("who", who(w)),
                ("claim", Value::from(claim.to_string())),
                ("root", Value::from(root.as_str())),
                ("keys", addresses_value(addresses)),
                ("note", note.as_deref().map_or(Value::Null, Value::from)),
                ("hold", Value::from(*hold)),
                ("expires_ms", Value::from(*expires_ms)),
                ("version", Value::from(*version)),
                ("actor", actor(w, a)),
                ("started_ms", Value::from(*started_ms)),
            ])
        }
        // `keys` (kit v0.19.0): a claim on elements names them by address as
        // they were when it was taken, so a view can say what went (`how:
        // "gone_outside_save"` when an outside save took them out). Only when
        // there are some, as the kit's WIRE.md and goldens: a claim on lines
        // or cells has none, and its bytes are as before.
        RosterEvent::ClaimEnded { who: w, claim, how, lines: l, version, actor: a, at_ms, at_rev, addresses, .. } => {
            let mut fields = vec![
                ("kind", Value::from("claim_ended")),
                ("who", who(w)),
                ("claim", Value::from(claim.to_string())),
                ("how", Value::from(how.as_str())),
                ("lines", lines(l)),
                ("version", Value::from(*version)),
                ("actor", actor(w, a)),
                ("at_ms", Value::from(*at_ms)),
                ("at_rev", at_rev.map_or(Value::Null, Value::from)),
            ];
            if !addresses.is_empty() {
                fields.push(("keys", addresses_value(addresses)));
            }
            Value::obj(fields)
        }
        // Wire 3: element presence (a keyed doc's viewers and agents).
        RosterEvent::Selection { who: w, selection: s, client, .. } => {
            elem_event("selection", w, s.as_ref().map_or(Value::Null, selection), client)
        }
        RosterEvent::Pointer { who: w, pointer: p, client, .. } => elem_event("pointer", w, p.as_ref().map_or(Value::Null, point), client),
        RosterEvent::Viewport { who: w, viewport: r, client, .. } => elem_event("viewport", w, r.as_ref().map_or(Value::Null, rect), client),
        RosterEvent::Focus { who: w, focus: f, client, .. } => elem_event("focus", w, f.as_ref().map_or(Value::Null, focus), client),
        RosterEvent::Range { who: w, range: g, client, .. } => elem_event("range", w, g.as_ref().map_or(Value::Null, range), client),
        // Kit v0.18.0: the view a connection looks at, by address, and its
        // playhead (`at_ms` stamped by the kit when it took it).
        RosterEvent::ViewAddress { who: w, address, client, .. } => {
            elem_event("view_address", w, address.as_deref().map_or(Value::Null, Value::from), client)
        }
        RosterEvent::TimePosition { who: w, time, client, .. } => {
            elem_event("time_position", w, time.as_ref().map_or(Value::Null, time_position), client)
        }
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

/// `presence_refused {req, error}` (WIRE.md "Presence frames"): a
/// presence frame the kit refused; nothing changed. Numbered by the
/// channel's own `req`, never an `op`.
pub fn presence_refused_frame(req: u64, e: &ErrorReport) -> Value {
    Value::obj([("type", Value::from("presence_refused")), ("req", Value::from(req)), ("error", e.to_value())])
}

/// `claimed {req, claim}` (WIRE.md "Claim frames"): the claim's id.
pub fn claimed_frame(req: u64, claim: &str) -> Value {
    Value::obj([("type", Value::from("claimed")), ("req", Value::from(req)), ("claim", Value::from(claim))])
}

/// `released {req, claims}`: the ids of the claims released.
pub fn released_frame(req: u64, claims: &[String]) -> Value {
    Value::obj([
        ("type", Value::from("released")),
        ("req", Value::from(req)),
        ("claims", Value::List(claims.iter().map(|c| Value::from(c.as_str())).collect())),
    ])
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

/// The first frame: `welcome {root, version, text?, client, self, rights,
/// wire}` (`wire`: 3, this host's [`WIRE`]), then K2's extras (`core`,
/// `family`, `path`, `turn_idle_ms`: the view's fallback for a missed
/// `turn_end`, [`super::turn::TURN_IDLE_MS`]).
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
        ("wire", Value::from(WIRE)),
        ("core", Value::from(core)),
        ("family", Value::from(family)),
        ("path", Value::from(path)),
        ("turn_idle_ms", Value::from(super::turn::TURN_IDLE_MS)),
    ] {
        o.push((k.to_string(), v));
    }
    Value::Obj(o)
}

// ── Client frames ────────────────────────────────────────────────────

/// A frame from the client (WIRE.md "Client → host").
#[derive(Debug, Clone, PartialEq)]
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
    /// K2's own: the app showed this person their removed-text notes
    /// (`overwritten`), so they're delivered (`Session::take_notes`) and no
    /// later snapshot or verb repeats them. No reply.
    TakeNotes,
    /// `watch` (the keyed watch, feature `keyed_watch`): open the watch if
    /// there is none, then a fresh snapshot ([`super::live_watch`]).
    Watch,
    /// `report {seq}`: the last watch frame the client applied (taken,
    /// unused on this engine).
    Report { seq: u64 },
    /// `view_address {req, address}` (feature `view_address`, kit
    /// v0.18.0): the view this connection looks at, or `None` to clear it.
    ViewAddress { req: u64, address: Option<String> },
    /// `time_position {req, time_position}` (feature `time_position`):
    /// this connection's playhead, or `None` to clear it. Read as sent;
    /// the kit checks the values ([`TimeSet::position`]).
    TimePosition { req: u64, time: Option<TimeSet> },
    /// `claim {req, root?, keys, ttl_ms?, note?, hold?}` (feature `claims`,
    /// WIRE.md "Claim frames"): claim elements of a list root by their keys,
    /// or, with no `root`, by their addresses (`@obj:5`).
    Claim { req: u64, root: Option<String>, keys: Vec<String>, ttl_ms: Option<u64>, note: Option<String>, hold: bool },
    /// `release {req, claim?}` (feature `claims`): one claim, or (none)
    /// every claim of this participant's here.
    Release { req: u64, claim: Option<String> },
    /// `select {req, selection: {root?, keys} | null}` (feature `presence`,
    /// WIRE.md "Presence frames"): elements by key under `root`, or by
    /// address with no `root`.
    Select { req: u64, selection: Option<(Option<String>, Vec<String>)> },
    /// `pointer {req, point: {x, y} | null}` (feature `presence`).
    Pointer { req: u64, point: Option<(f64, f64)> },
    /// `viewport {req, rect: {x, y, w, h} | null}` (feature `presence`).
    Viewport { req: u64, rect: Option<(f64, f64, f64, f64)> },
}

/// A `time_position` frame's value as the client sent it: `{at, state,
/// in?, rate?}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeSet {
    pub at: String,
    pub state: String,
    pub within: Option<String>,
    pub rate: Option<String>,
}

impl TimeSet {
    /// The kit's value (`TimePosition::new`, then `.within` and `.rate`
    /// when given). A value the kit won't take is `MPC-E114`
    /// (`BadPosition`), as the kit's own setters say.
    pub fn position(&self) -> Result<TimePosition, SessionError> {
        let state = PlayState::parse(&self.state)
            .ok_or_else(|| SessionError::BadPosition(format!("the state {:?} is not playing or paused", self.state)))?;
        let mut t = TimePosition::new(&self.at, state)?;
        if let Some(a) = &self.within {
            t = t.within(a)?;
        }
        if let Some(r) = &self.rate {
            t = t.rate(r)?;
        }
        Ok(t)
    }
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
        "take_notes" => ClientFrame::TakeNotes,
        "watch" => ClientFrame::Watch,
        "report" => ClientFrame::Report { seq: u64_field(o, "seq").map_err(bad)? },
        "view_address" => ClientFrame::ViewAddress {
            req: u64_field(o, "req").map_err(bad)?,
            address: match o.get("address") {
                Some(Json::Null) => None,
                Some(Json::String(a)) => Some(a.clone()),
                _ => return Err(bad("`address` must be a string or null".into())),
            },
        },
        "time_position" => ClientFrame::TimePosition {
            req: u64_field(o, "req").map_err(bad)?,
            time: match o.get("time_position") {
                Some(Json::Null) => None,
                Some(Json::Object(t)) => {
                    let opt = |k: &str| -> Result<Option<String>, String> {
                        match t.get(k) {
                            None | Some(Json::Null) => Ok(None),
                            Some(Json::String(s)) => Ok(Some(s.clone())),
                            Some(_) => Err(format!("`time_position.{k}` must be a string")),
                        }
                    };
                    Some(TimeSet {
                        at: str_field(t, "at").map_err(|m| bad(format!("time_position: {m} (a decimal integer string)")))?,
                        state: str_field(t, "state").map_err(|m| bad(format!("time_position: {m}")))?,
                        within: opt("in").map_err(bad)?,
                        rate: opt("rate").map_err(bad)?,
                    })
                }
                _ => return Err(bad("`time_position` must be an object {at, state, in?, rate?} or null".into())),
            },
        },
        "claim" => ClientFrame::Claim {
            req: u64_field(o, "req").map_err(bad)?,
            root: match o.get("root") {
                None | Some(Json::Null) => None,
                Some(Json::String(s)) => Some(s.clone()),
                Some(_) => return Err(bad("`root` must be a string".into())),
            },
            keys: match o.get("keys") {
                Some(Json::Array(a)) => a
                    .iter()
                    .map(|k| k.as_str().map(str::to_string).ok_or_else(|| bad("`keys` must be an array of strings".into())))
                    .collect::<Result<Vec<_>, _>>()?,
                _ => return Err(bad("`keys` must be an array of strings".into())),
            },
            ttl_ms: match o.get("ttl_ms") {
                None | Some(Json::Null) => None,
                Some(n) => Some(n.as_u64().ok_or_else(|| bad("`ttl_ms` must be a whole number".into()))?),
            },
            note: match o.get("note") {
                None | Some(Json::Null) => None,
                Some(Json::String(s)) => Some(s.clone()),
                Some(_) => return Err(bad("`note` must be a string".into())),
            },
            hold: match o.get("hold") {
                None | Some(Json::Null) => false,
                Some(Json::Bool(b)) => *b,
                Some(_) => return Err(bad("`hold` must be a boolean".into())),
            },
        },
        "select" => ClientFrame::Select {
            req: u64_field(o, "req").map_err(bad)?,
            selection: match o.get("selection") {
                Some(Json::Null) => None,
                Some(Json::Object(s)) => Some((
                    match s.get("root") {
                        None | Some(Json::Null) => None,
                        Some(Json::String(r)) => Some(r.clone()),
                        Some(_) => return Err(bad("`selection.root` must be a string".into())),
                    },
                    match s.get("keys") {
                        Some(Json::Array(a)) => a
                            .iter()
                            .map(|k| k.as_str().map(str::to_string).ok_or_else(|| bad("`selection.keys` must be an array of strings".into())))
                            .collect::<Result<Vec<_>, _>>()?,
                        _ => return Err(bad("`selection.keys` must be an array of strings".into())),
                    },
                )),
                _ => return Err(bad("`selection` must be an object {root?, keys} or null".into())),
            },
        },
        "pointer" => ClientFrame::Pointer {
            req: u64_field(o, "req").map_err(bad)?,
            point: match o.get("point") {
                Some(Json::Null) => None,
                Some(Json::Object(p)) => {
                    let n = |k: &str| p.get(k).and_then(Json::as_f64).ok_or_else(|| bad(format!("`point.{k}` must be a number")));
                    Some((n("x")?, n("y")?))
                }
                _ => return Err(bad("`point` must be an object {x, y} or null".into())),
            },
        },
        "viewport" => ClientFrame::Viewport {
            req: u64_field(o, "req").map_err(bad)?,
            rect: match o.get("rect") {
                Some(Json::Null) => None,
                Some(Json::Object(p)) => {
                    let n = |k: &str| p.get(k).and_then(Json::as_f64).ok_or_else(|| bad(format!("`rect.{k}` must be a number")));
                    Some((n("x")?, n("y")?, n("w")?, n("h")?))
                }
                _ => return Err(bad("`rect` must be an object {x, y, w, h} or null".into())),
            },
        },
        "release" => ClientFrame::Release {
            req: u64_field(o, "req").map_err(bad)?,
            claim: match o.get("claim") {
                None | Some(Json::Null) => None,
                Some(Json::String(s)) => Some(s.clone()),
                Some(_) => return Err(bad("`claim` must be a string".into())),
            },
        },
        other => return Err(bad(format!("unknown frame type `{other}`"))),
    })
}

#[cfg(test)]
#[path = "live_wire_golden_tests.rs"]
mod golden_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// The claim frames (feature `claims`): read as sent; a bad field is
    /// the frame's refusal. Their replies by `req`.
    #[test]
    fn claim_and_release_frames_parse_and_reply_by_req() {
        assert_eq!(
            parse(r#"{"type":"claim","req":4,"root":"objs","keys":["o3","o5"],"ttl_ms":60000,"note":"moving the lamp","hold":true}"#),
            Ok(ClientFrame::Claim { req: 4, root: Some("objs".into()), keys: vec!["o3".into(), "o5".into()], ttl_ms: Some(60_000), note: Some("moving the lamp".into()), hold: true })
        );
        assert_eq!(
            parse(r#"{"type":"claim","req":5,"keys":["@obj:3"]}"#),
            Ok(ClientFrame::Claim { req: 5, root: None, keys: vec!["@obj:3".into()], ttl_ms: None, note: None, hold: false })
        );
        assert_eq!(parse(r#"{"type":"release","req":6}"#), Ok(ClientFrame::Release { req: 6, claim: None }));
        assert_eq!(parse(r#"{"type":"release","req":7,"claim":"claim-2"}"#), Ok(ClientFrame::Release { req: 7, claim: Some("claim-2".into()) }));
        for bad in [r#"{"type":"claim","req":1}"#, r#"{"type":"claim","req":1,"keys":[3]}"#, r#"{"type":"claim","keys":[]}"#, r#"{"type":"claim","req":1,"keys":[],"hold":"yes"}"#, r#"{"type":"release","req":1,"claim":2}"#] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert_eq!(
            parse(r#"{"type":"select","req":8,"selection":{"keys":["@obj:3"]}}"#),
            Ok(ClientFrame::Select { req: 8, selection: Some((None, vec!["@obj:3".into()])) })
        );
        assert_eq!(parse(r#"{"type":"select","req":9,"selection":null}"#), Ok(ClientFrame::Select { req: 9, selection: None }));
        assert_eq!(parse(r#"{"type":"pointer","req":10,"point":{"x":0.5,"y":2}}"#), Ok(ClientFrame::Pointer { req: 10, point: Some((0.5, 2.0)) }));
        assert_eq!(parse(r#"{"type":"viewport","req":11,"rect":{"x":0,"y":0,"w":10,"h":5}}"#), Ok(ClientFrame::Viewport { req: 11, rect: Some((0.0, 0.0, 10.0, 5.0)) }));
        for bad in [r#"{"type":"select","req":1}"#, r#"{"type":"pointer","req":1,"point":{"x":1}}"#, r#"{"type":"viewport","req":1,"rect":{"x":0,"y":0,"w":"a","h":1}}"#] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert_eq!(claimed_frame(4, "claim-9").to_json(), r#"{"type":"claimed","req":4,"claim":"claim-9"}"#);
        assert_eq!(released_frame(6, &["claim-9".into()]).to_json(), r#"{"type":"released","req":6,"claims":["claim-9"]}"#);
    }

    /// Kit v0.16.0: element presence goes on the wire by address (a deck's
    /// `@sld:256/sp:7`), never by the store's key; a pack that maps no
    /// addresses gives its keys, so those bytes don't change.
    #[test]
    fn element_presence_is_sent_by_address() {
        let key = |k: &str| k2_mpc_base::Key::new(k).expect("key");
        let mut sel = ElemSelection::new("shapes", vec![key("s256-7"), key("s256-9")]);
        assert_eq!(selection(&sel).to_json(), r#"{"root":"shapes","keys":["s256-7","s256-9"]}"#, "addresses start as the keys");
        sel.addresses = vec!["@sld:256/sp:7".into(), "@sld:256/sp:9".into()];
        assert_eq!(selection(&sel).to_json(), r#"{"root":"shapes","keys":["@sld:256/sp:7","@sld:256/sp:9"]}"#);
        let mut f = ElemFocus::new("slides", key("s256"));
        f.address = "@sld:256".into();
        assert_eq!(focus(&f).to_json(), r#"{"root":"slides","key":"@sld:256"}"#);
        let mut r = ElemRange::new("shapes", key("s256-7"), "text", 1, 4);
        r.address = "@sld:256/sp:7".into();
        let v = range(&r).to_json();
        assert!(v.contains(r#""key":"@sld:256/sp:7""#) && !v.contains("s256-7"), "{v}");
    }

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
        assert_eq!(parse(r#"{"type":"take_notes"}"#), Ok(ClientFrame::TakeNotes));
        assert_eq!(parse(r#"{"type":"watch"}"#), Ok(ClientFrame::Watch));
        assert_eq!(parse(r#"{"type":"report","seq":64}"#), Ok(ClientFrame::Report { seq: 64 }));
        assert!(parse(r#"{"type":"report"}"#).is_err(), "a report names its seq");
        // Kit v0.18.0 presence: numbered by `req` (never `op`), a value or null.
        assert_eq!(
            parse(r#"{"type":"view_address","req":2,"address":"@cam:c1"}"#),
            Ok(ClientFrame::ViewAddress { req: 2, address: Some("@cam:c1".into()) })
        );
        assert_eq!(parse(r#"{"type":"view_address","req":3,"address":null}"#), Ok(ClientFrame::ViewAddress { req: 3, address: None }));
        assert_eq!(
            parse(r#"{"type":"time_position","req":4,"time_position":{"at":"-1270080000","state":"paused","in":"@seq:main","rate":"1001/1000"}}"#),
            Ok(ClientFrame::TimePosition {
                req: 4,
                time: Some(TimeSet {
                    at: "-1270080000".into(),
                    state: "paused".into(),
                    within: Some("@seq:main".into()),
                    rate: Some("1001/1000".into())
                })
            })
        );
        assert_eq!(parse(r#"{"type":"time_position","req":5,"time_position":null}"#), Ok(ClientFrame::TimePosition { req: 5, time: None }));
        for junk in [
            r#"{"type":"view_address","req":6}"#,
            r#"{"type":"view_address","address":"@cam:c1"}"#,
            r#"{"type":"view_address","req":6,"address":7}"#,
            r#"{"type":"time_position","req":7,"time_position":{"at":12,"state":"paused"}}"#,
            r#"{"type":"time_position","req":7,"time_position":{"at":"12"}}"#,
            r#"{"type":"time_position","req":7}"#,
        ] {
            let b = parse(junk).expect_err(junk);
            assert_eq!(b.op, None, "a presence frame has no op, so its junk is an `error` frame: {junk}");
        }

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

    /// A value the kit won't take is the kit's `MPC-E114` (`BadPosition`),
    /// answered `presence_refused {req, error}`, never `refused {op}`.
    #[test]
    fn a_time_position_is_the_kits_value_or_its_bad_position() {
        let t = TimeSet { at: "254016000000".into(), state: "playing".into(), within: Some("@seq:main".into()), rate: None };
        let p = t.position().expect("a good playhead");
        assert_eq!((p.at(), p.state(), p.in_address(), p.rate_text()), ("254016000000", PlayState::Playing, Some("@seq:main"), None));
        assert_eq!(time_position(&p).to_json(), r#"{"at":"254016000000","state":"playing","at_ms":0,"in":"@seq:main"}"#);
        for bad in [
            TimeSet { at: "1.5".into(), state: "paused".into(), within: None, rate: None },
            TimeSet { at: "007".into(), state: "paused".into(), within: None, rate: None },
            TimeSet { at: "1".into(), state: "rewinding".into(), within: None, rate: None },
            TimeSet { at: "1".into(), state: "paused".into(), within: None, rate: Some("1/0".into()) },
            TimeSet { at: "1".into(), state: "paused".into(), within: Some(String::new()), rate: None },
        ] {
            let e = bad.position().expect_err("refused");
            assert_eq!(e.report().id(), "MPC-E114", "{bad:?}: {e}");
            let f = presence_refused_frame(9, &e.report()).to_json();
            assert!(f.starts_with(r#"{"type":"presence_refused","req":9,"error":{"id":"MPC-E114""#), "{f}");
        }
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
            (CloseReason::RolledBack, "rolled_back", true),
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
