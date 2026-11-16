//! The timeline sample's watched turn (`tests/watch_turn.rs`), made only by the pack's verbs and
//! the kit's own frames, and the rows that hold it to its golden
//! (`tests/watch_turn_golden/trim_held_outside_save.json`), shared with the planted bugs
//! (`tests/mutants.rs`).
//!
//! **The turn**, on [`cut`] (the live rows' timeline, written by this crate's own generator), as
//! a person (`ada`) watches it by key and follows the roster:
//!
//! 1. `watch`: the keyed watch's snapshot and the roster's;
//! 2. `claim`: an agent (`atlas`) claims Shot A (`@item:1`) with `hold`;
//! 3. `outside_save`: an app saves the file with another clip renamed (Shot C), taken in by key
//!    while the hold is live;
//! 4. to 8. the agent's verbs on the clip it holds, under one turn: a trim of its start, a move, a
//!    rename, `clip enable --off` and a marker on it, each change frame carrying its claim;
//! 9. `refused_trim`: a second agent (`nova`) trims the held clip and is refused (the host
//!    respects claims: `ClaimPolicy::Respect`), the refusal naming the holder and the claim;
//! 10. `write_back`: the live changes written to the file;
//! 11. `release`: the agent lets the clip go.
//!
//! The outside save comes before the agent's edits: the kit takes a save in by key only onto a
//! doc whose changes are all in the file, with no write of its own since the last outside save.
//! On unwritten changes, or after a write-back, a save that differs keeps both (the kit's
//! `docs/SESSIONS.md`, "Outside saves on a keyed doc that isn't a table").
//!
//! **What the golden holds**, per step in order: its name, who acted (`by`), the verb's
//! arguments and exit (`null` for a call that is no verb), every frame the person's keyed watch
//! got for it (`WatchFrame::to_value`, the kit's own JSON) and every roster event the person got
//! for it (encoded as the kit's pinned wire encoder encodes them: [`roster_event`]). Only what is
//! random per run is rewritten: each client ID becomes a fixed number in the order it first
//! appears (the three participants as they join, then any other), each claim id `claim-N` in the
//! order it first appears. The host's clock is fixed and moves by fixed amounts.
//!
//! **The plant** `TurnRefusalDropped` (`--features mutants`) drops the refusal frame from what the
//! recorder keeps: the golden row must fail at the golden comparison.
#![allow(dead_code)]

#[path = "../kit_view_golden/mod.rs"]
mod kit_view_golden;

use std::collections::HashMap;
use std::sync::mpsc::Receiver;

use k2_mpc_base::errors::Value;
use k2_mpc_base::session::{
    ClaimPolicy, ClaimSpan, ConnId, CursorAt, DiskCheck, ElemFocus, ElemRange, ElemSelection,
    KeyedWatch, LeaveReason, LineRange, Participant, ParticipantKind, Point, Rect, Rights, Role,
    RosterEvent, RosterMember, Session, Stay, TimePosition, Turn, WatchFrame, WatchOptions, Who,
    KEYED_WATCH_FEATURE,
};
use k2_mpc_base::store::Seq;
use k2_mpc_base::tools::ToolOptions;
use k2_mpc_timeline::live::ITEMS;
use k2_mpc_timeline::mutant;

use crate::live_support::{app_save, args, cut, cut_clips, cut_tree, file, fx, run, stdout, Fx};

/// The golden's name, in [`DIR`].
pub const GOLDEN: &str = "trim_held_outside_save";

/// The goldens' folder, under `tests/`.
const DIR: &str = "watch_turn_golden";

/// The plant: the recorder drops the refusal frame.
pub const PLANT: &str = "TurnRefusalDropped";

/// The clip the agent holds: Shot A.
const HELD: &str = "@item:1";

/// The host's clock moves this much before each step's call.
const STEP_MS: u64 = 1_000;

/// And this much after it, past the roster's interval (the kit's default, 250 ms), so the
/// roster's flush delivers what the call made.
const ROSTER_MS: u64 = 300;

/// The hold's time: longer than the turn.
const CLAIM_MS: u64 = 600_000;

/// The holder's turn, and the refused agent's.
const ATLAS_TURN: &str = "turn-1";
const NOVA_TURN: &str = "turn-2";

/// The holder's verbs on the clip it holds, in order, each named as its step.
const HOLDER_VERBS: &[(&str, &[&str])] = &[
    // its start six frames later (record start and source in)
    ("trim", &["clip", "trim", HELD, "--in", "+6f"]),
    // six frames earlier, back onto the frame it started at
    ("move", &["clip", "move", HELD, "--to", "-6f"]),
    ("rename", &["clip", "rename", HELD, "Shot A tight"]),
    ("off", &["clip", "enable", HELD, "--off"]),
    (
        "marker",
        &[
            "marker",
            "add",
            "--on",
            HELD,
            "--at",
            "01:00:01:12",
            "--name",
            "cue",
        ],
    ),
];

/// The second agent's trim of the held clip's end.
const REFUSED_TRIM: &[&str] = &["clip", "trim", HELD, "--out", "-2f"];

// ---------- the rows ----------

/// The turn is the golden, byte for byte.
pub fn row_turn_is_the_golden() -> Result<(), String> {
    const ROW: &str = "row_turn_is_the_golden";
    let got = turn("golden").map_err(|e| format!("{ROW}: {e}"))?;
    kit_view_golden::check_in(DIR, GOLDEN, &got).map_err(|e| format!("{ROW}: {e}"))
}

/// The turn twice, in two fresh fixtures: the same JSON, byte for byte. Its length.
pub fn row_turn_is_reproducible() -> Result<usize, String> {
    const ROW: &str = "row_turn_is_reproducible";
    let a = turn("again-a")
        .map_err(|e| format!("{ROW}: {e}"))?
        .to_json_pretty();
    let b = turn("again-b")
        .map_err(|e| format!("{ROW}: {e}"))?
        .to_json_pretty();
    if a != b {
        let at = a
            .bytes()
            .zip(b.bytes())
            .position(|(x, y)| x != y)
            .unwrap_or(a.len().min(b.len()));
        let from = at.saturating_sub(200);
        return Err(format!(
            "{ROW}: two runs differ at byte {at}:\n--- first:\n{}\n--- second:\n{}",
            a.get(from..(at + 200).min(a.len())).unwrap_or(""),
            b.get(from..(at + 200).min(b.len())).unwrap_or("")
        ));
    }
    if a.is_empty() {
        return Err(format!("{ROW}: the turn is empty"));
    }
    Ok(a.len())
}

/// What the turn holds, counted.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub steps: usize,
    pub frames: usize,
    pub roster_events: usize,
    pub changes: usize,
    /// Change frames of the holder's trim of the held clip, under its claim.
    pub held_trims: usize,
    /// Refusal frames naming the holder and its claim.
    pub refusals: usize,
    /// Outside save frames taken in.
    pub ingested: usize,
    pub claim_started_elems: usize,
    pub claim_ended: usize,
}

/// The turn holds each thing it is for: the holder's trim under its claim, the refusal naming
/// the holder and the claim, the outside save taken in, the claim's start and its end. An empty
/// or partial turn fails.
pub fn row_turn_holds_each_frame_kind() -> Result<Counts, String> {
    const ROW: &str = "row_turn_holds_each_frame_kind";
    let got = turn("counts").map_err(|e| format!("{ROW}: {e}"))?;
    let c = count(&got);
    let want = [
        (
            c.held_trims,
            "an ops_change of the holder's trim under its claim",
        ),
        (c.refusals, "a verb_refused naming the holder and its claim"),
        (c.ingested, "an outside_save taken in"),
        (c.claim_started_elems, "a claim_started_elems"),
        (c.claim_ended, "a claim_ended"),
    ];
    for (n, what) in want {
        if n == 0 {
            return Err(format!("{ROW}: the turn holds no {what}: {c:?}"));
        }
    }
    if c.steps != 11 {
        return Err(format!("{ROW}: {} steps, want 11: {c:?}", c.steps));
    }
    Ok(c)
}

fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut at = v;
    for k in path {
        at = at.get(k)?;
    }
    at.as_str()
}

fn count(turn: &Value) -> Counts {
    let mut c = Counts::default();
    let Some(Value::List(steps)) = turn.get("steps") else {
        return c;
    };
    c.steps = steps.len();
    for s in steps {
        if let Some(Value::List(frames)) = s.get("watch") {
            c.frames += frames.len();
            for f in frames {
                let kind = str_at(f, &["type"]).unwrap_or("");
                let claimed = str_at(f, &["claim"]).is_some_and(|x| x.starts_with("claim-"));
                if kind == "ops_change" {
                    c.changes += 1;
                }
                if kind == "ops_change"
                    && str_at(f, &["verb"]) == Some("clip trim")
                    && str_at(f, &["actor", "id"]) == Some("atlas")
                    && claimed
                {
                    c.held_trims += 1;
                }
                if kind == "verb_refused"
                    && str_at(f, &["holder", "id"]) == Some("atlas")
                    && claimed
                {
                    c.refusals += 1;
                }
                if kind == "outside_save" && str_at(f, &["outcome"]) == Some("ingested") {
                    c.ingested += 1;
                }
            }
        }
        if let Some(Value::List(events)) = s.get("roster") {
            c.roster_events += events.len();
            for e in events {
                match str_at(e, &["kind"]) {
                    Some("claim_started_elems") => c.claim_started_elems += 1,
                    Some("claim_ended") => c.claim_ended += 1,
                    _ => {}
                }
            }
        }
    }
    c
}

// ---------- the turn ----------

/// The turn in a fresh fixture (`tag` names its folder), as the golden holds it.
pub fn turn(tag: &str) -> Result<Value, String> {
    let f = fx(&format!("turn-{tag}"), |_| {});
    f.write(&cut());
    let s = f.open();
    let mut st = Stable::default();
    let ada = join(
        &s,
        &mut st,
        "ada",
        "Ada",
        "#7c3aed",
        ParticipantKind::Person,
    )?;
    let atlas = join(
        &s,
        &mut st,
        "atlas",
        "Atlas",
        "#0ea5e9",
        ParticipantKind::Agent,
    )?;
    let nova = join(
        &s,
        &mut st,
        "nova",
        "Nova",
        "#c2410c",
        ParticipantKind::Agent,
    )?;

    // 1. the person's keyed watch and roster
    let mut w = s
        .watch_keyed(ada, &WatchOptions::new(&[KEYED_WATCH_FEATURE]))
        .map_err(|e| format!("the watch: {e}"))?
        .ok_or("the timeline is not watchable by key")?;
    let snapshot = s
        .watch_snapshot(&mut w)
        .map_err(|e| format!("the snapshot: {e}"))?;
    let roster = s
        .subscribe_roster(ada)
        .map_err(|e| format!("the roster: {e}"))?;
    let mut r = Recorder {
        f: &f,
        s: &s,
        w,
        roster,
        st,
        steps: Vec::new(),
    };
    r.step("watch", "ada", None, None, snapshot)?;

    // 2. the hold
    f.advance(STEP_MS);
    let claim = s
        .claim_elems_at(
            atlas,
            &[HELD.to_owned()],
            Some(CLAIM_MS),
            Some("trimming the head".to_owned()),
            true,
        )
        .map_err(|e| format!("the claim: {e}"))?;
    r.step("claim", "atlas", None, None, Vec::new())?;

    // 3. an app's save: another clip renamed, taken in by key while the hold is live
    f.advance(STEP_MS);
    let mut clips = cut_clips();
    clips[2].name = "Shot C graded".into();
    app_save(&f.path(), &file(&cut_tree(&clips)));
    let out = s
        .file_changed()
        .map_err(|e| format!("the outside save: {e}"))?;
    if !matches!(out, DiskCheck::Ingested { .. }) {
        return Err(format!("the outside save was not taken in: {out:?}"));
    }
    r.step("outside_save", "app", None, None, Vec::new())?;

    // 4. to 8. the holder's verbs on the clip it holds
    for (name, verb) in HOLDER_VERBS {
        f.advance(STEP_MS);
        let (argv, exit, said) = based(&s, atlas, verb, ATLAS_TURN)?;
        if exit != 0 {
            return Err(format!("{name}: {argv:?} exited {exit}: {said}"));
        }
        r.step(name, "atlas", Some(argv), Some(exit), Vec::new())?;
    }

    // 9. the second agent's trim of the held clip, refused
    f.advance(STEP_MS);
    let (argv, exit, said) = based(&s, nova, REFUSED_TRIM, NOVA_TURN)?;
    if exit != 5 {
        return Err(format!(
            "the second agent's trim: {argv:?} exited {exit}, want 5 (claimed): {said}"
        ));
    }
    r.step("refused_trim", "nova", Some(argv), Some(exit), Vec::new())?;

    // 10. the write-back
    f.advance(STEP_MS);
    let out = s
        .flush(atlas)
        .map_err(|e| format!("the write-back failed: {e}"))?;
    if !out.written {
        return Err(format!("the write-back wrote nothing: {out:?}"));
    }
    r.step("write_back", "atlas", None, None, Vec::new())?;

    // 11. the hold let go
    f.advance(STEP_MS);
    let gone = s
        .release_claim(atlas, Some(claim.id))
        .map_err(|e| format!("the release: {e}"))?;
    if gone != [claim.id] {
        return Err(format!("the release ended {gone:?}, want [{}]", claim.id));
    }
    r.step("release", "atlas", None, None, Vec::new())?;

    Ok(Value::obj([
        ("doc", Value::from(crate::live_support::NAME)),
        ("steps", Value::List(r.steps)),
    ]))
}

/// `pid` joins `s` (held, via the CLI), its client registered for [`Stable`].
fn join(
    s: &Session,
    st: &mut Stable,
    pid: &str,
    name: &str,
    colour: &str,
    kind: ParticipantKind,
) -> Result<ConnId, String> {
    let j = s
        .join(Participant::new(pid, name, colour, kind, "cli"), Stay::Held)
        .map_err(|e| format!("{pid} joins: {e}"))?;
    st.client(j.client);
    Ok(j.conn)
}

/// An agent's edit verb as a host runs it: on the version of a read it just made (`--base`),
/// under the host's claim policy (others' claims respected) and turn. Its arguments, its exit,
/// and what it printed or why it was refused.
fn based(
    s: &Session,
    c: ConnId,
    verb: &[&str],
    turn: &str,
) -> Result<(Vec<String>, u8, String), String> {
    let v = run(s, c, &["timeline", "info"])
        .map_err(|e| format!("the read before {verb:?}: {e}"))?
        .version;
    let mut argv = args(verb);
    argv.push("--base".into());
    argv.push(v.to_string());
    let turn = Turn::new(turn).map_err(|e| format!("{turn}: {e}"))?;
    let opts = ToolOptions::new().policy(ClaimPolicy::Respect).turn(turn);
    Ok(match s.run_tool_with(c, ITEMS, &argv, b"", &opts) {
        Ok(o) => {
            let said = stdout(&o);
            (argv, o.exit, said)
        }
        Err(e) => {
            let said = e.to_string();
            (argv, e.exit(), said)
        }
    })
}

/// What the person got, step by step.
struct Recorder<'a> {
    f: &'a Fx,
    s: &'a Session,
    w: KeyedWatch,
    roster: Receiver<RosterEvent>,
    st: Stable,
    steps: Vec<Value>,
}

impl Recorder<'_> {
    /// The step just made: the frames before it (`first`, the snapshot's) and every frame and
    /// roster event the person got for it, after the roster's interval.
    fn step(
        &mut self,
        name: &str,
        by: &str,
        argv: Option<Vec<String>>,
        exit: Option<u8>,
        first: Vec<WatchFrame>,
    ) -> Result<(), String> {
        self.f.advance(ROSTER_MS);
        self.s
            .flush_roster()
            .map_err(|e| format!("{name}: the roster's flush: {e}"))?;
        let mut frames = first;
        frames.extend(self.w.try_frames());
        if mutant::active(PLANT) {
            // planted: the refusal frame isn't kept
            frames.retain(|f| !matches!(f, WatchFrame::Refused(_)));
        }
        let watch: Vec<Value> = frames.iter().map(|f| self.st.frame(f)).collect();
        let mut roster = Vec::new();
        for e in self.roster.try_iter() {
            roster.push(roster_event(&e, &mut self.st));
        }
        self.steps.push(Value::obj([
            ("step", Value::from(name)),
            ("by", Value::from(by)),
            ("argv", argv.as_deref().map_or(Value::Null, strings)),
            (
                "exit",
                exit.map_or(Value::Null, |x| Value::from(u64::from(x))),
            ),
            ("watch", Value::List(watch)),
            ("roster", Value::List(roster)),
        ]));
        Ok(())
    }
}

/// Arguments as JSON strings.
fn strings(a: &[String]) -> Value {
    Value::List(a.iter().map(|x| Value::from(x.as_str())).collect())
}

/// Fixed numbers for the run's random client IDs, and claim ids, each in the order it first
/// appears (the kit's wire golden's `Stable`, its clients numbered as they appear).
#[derive(Default)]
struct Stable {
    clients: HashMap<u64, u64>,
    claims: HashMap<u64, u64>,
}

impl Stable {
    fn client(&mut self, c: u64) -> u64 {
        let n = self.clients.len() as u64 + 1;
        *self.clients.entry(c).or_insert(n)
    }
    fn claim(&mut self, id: u64) -> String {
        let n = self.claims.len() as u64 + 1;
        format!("claim-{}", self.claims.entry(id).or_insert(n))
    }

    /// A claim as the kit prints it (`claim-N`), numbered.
    fn claim_named(&mut self, c: &str) -> String {
        match c.strip_prefix("claim-").and_then(|n| n.parse().ok()) {
            Some(id) => self.claim(id),
            None => panic!("a claim the kit printed as {c}"),
        }
    }

    /// A watch frame as the kit prints it, its `client` and `claim` made stable.
    fn frame(&mut self, f: &WatchFrame) -> Value {
        let mut v = f.to_value();
        if let Value::Obj(m) = &mut v {
            for (k, x) in m.iter_mut() {
                let stable = match (k.as_str(), &*x) {
                    ("client", Value::UInt(c)) => Some(Value::from(self.client(*c))),
                    ("claim", Value::Str(c)) => Some(Value::from(self.claim_named(c))),
                    _ => None,
                };
                if let Some(v) = stable {
                    *x = v;
                }
            }
        }
        v
    }
}

// ---------- the roster's JSON ----------
//
// The kit has no roster encoder: the host encodes `RosterEvent`. These mirror the kit's pinned
// wire golden encoder (k2-mpc-base, `crates/base/tests/wire_golden.rs`), function for function,
// for the kinds this turn can give; [`Stable::client`] numbers a client on first sight. Any other
// kind fails the run.

fn rights(r: Rights) -> &'static str {
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

fn reason(r: LeaveReason) -> &'static str {
    match r {
        LeaveReason::Left => "left",
        LeaveReason::Idle => "idle",
        LeaveReason::Kicked => "kicked",
        LeaveReason::RightsLost => "rights_lost",
    }
}

fn who(w: &Who) -> Value {
    Value::from(w.to_string())
}

fn lines(l: &LineRange) -> Value {
    Value::obj([
        ("root", Value::from(l.root.as_str())),
        ("first", Value::from(l.first)),
        ("last", Value::from(l.last)),
    ])
}

/// A caret: bytes of the root's text at `version`, and the kit's `line:col` for display.
fn caret(c: &CursorAt, version: Seq) -> Value {
    Value::obj([
        ("root", Value::from(c.cursor.root.as_str())),
        ("anchor", Value::from(c.cursor.anchor)),
        ("head", Value::from(c.cursor.head)),
        ("version", Value::from(version)),
        (
            "anchor_lc",
            Value::from(format!("{}:{}", c.anchor.line, c.anchor.col)),
        ),
        (
            "head_lc",
            Value::from(format!("{}:{}", c.head.line, c.head.col)),
        ),
    ])
}

/// `RosterMember` → the wire's `RosterEntry`.
fn member(m: &RosterMember, version: Seq, st: &mut Stable) -> Value {
    let p = &m.entry.participant;
    let mut fields = vec![
        ("handle", who(&m.who)),
        ("name", Value::from(p.name.as_str())),
        ("colour", Value::from(p.colour.as_str())),
        ("kind", Value::from(kind(p.kind))),
        ("role", Value::from(role(m.entry.role))),
        ("away", Value::from(m.entry.away)),
        (
            "cursor",
            m.entry
                .cursor
                .as_ref()
                .map_or(Value::Null, |c| caret(c, version)),
        ),
        ("rights", Value::from(rights(m.rights))),
    ];
    if !m.entry.devices.is_empty() {
        let devices = m.entry.devices.iter().map(|d| {
            let mut o = vec![
                ("client", Value::from(st.client(d.client))),
                (
                    "cursor",
                    d.cursor.as_ref().map_or(Value::Null, |c| caret(c, version)),
                ),
            ];
            if let Some(l) = &d.label {
                o.push(("label", Value::from(l.as_str())));
            }
            Value::obj(o)
        });
        fields.push(("devices", Value::List(devices.collect())));
    }
    elem_fields(
        &mut fields,
        &m.entry.selection,
        &m.entry.pointer,
        &m.entry.viewport,
        &m.entry.focus,
        &m.entry.range,
    );
    if let Some(c) = &m.entry.cells {
        fields.push(("cells", c.to_value()));
    }
    view_time_fields(&mut fields, &m.entry.view_address, &m.entry.time);
    if let Some(Value::List(devices)) = fields
        .iter_mut()
        .find(|(k, _)| *k == "devices")
        .map(|(_, v)| v)
    {
        for (o, d) in devices.iter_mut().zip(&m.entry.devices) {
            let Value::Obj(o) = o else { continue };
            let mut more = Vec::new();
            elem_fields(
                &mut more,
                &d.selection,
                &d.pointer,
                &d.viewport,
                &d.focus,
                &d.range,
            );
            if let Some(c) = &d.cells {
                more.push(("cells", c.to_value()));
            }
            view_time_fields(&mut more, &d.view_address, &d.time);
            o.extend(more.into_iter().map(|(k, v)| (k.to_owned(), v)));
        }
    }
    let elem_claims: Vec<Value> = m
        .entry
        .claims
        .iter()
        .filter_map(|c| match &c.span {
            ClaimSpan::Elems { root, .. } => Some(Value::obj([
                ("claim", Value::from(st.claim(c.id.get()))),
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
    // a member's cells claims (`cell_claims`), only when they hold some
    let cell_claims: Vec<Value> = m
        .entry
        .claims
        .iter()
        .filter_map(|c| match &c.span {
            ClaimSpan::CellRange { range, .. } => Some(Value::obj([
                ("claim", Value::from(st.claim(c.id.get()))),
                (
                    "container",
                    Value::from(c.addresses.first().map_or("", String::as_str)),
                ),
                ("ranges", Value::List(vec![range.to_value()])),
                ("note", c.note.as_deref().map_or(Value::Null, Value::from)),
                ("hold", Value::from(c.hold)),
                ("expires_ms", Value::from(c.expires_ms)),
                ("started_ms", Value::from(c.taken_ms)),
            ])),
            _ => None,
        })
        .collect();
    if !cell_claims.is_empty() {
        fields.push(("cell_claims", Value::List(cell_claims)));
    }
    Value::obj(fields)
}

/// A claim event's `actor` (wire 3): `{id, name, kind, colour}`, `id` the event's `who`.
fn actor(w: &Who, a: &k2_mpc_base::session::ClaimActor) -> Value {
    Value::obj([
        ("id", who(w)),
        ("name", Value::from(a.name.as_str())),
        ("kind", Value::from(kind(a.kind))),
        ("colour", Value::from(a.colour.as_str())),
    ])
}

/// Elements by address, as the wire's `keys` (the kit fills them, `Pack::address_of`).
fn addresses_value(addresses: &[String]) -> Value {
    Value::List(addresses.iter().map(|a| Value::from(a.as_str())).collect())
}

/// A coordinate: the turn uses whole numbers, which `errors::Value` encodes as they are.
fn coord(v: f64) -> Value {
    assert_eq!(v.fract(), 0.0, "the turn uses whole numbers");
    Value::Int(v as i64)
}

fn selection(s: &ElemSelection) -> Value {
    Value::obj([
        ("root", Value::from(s.root.as_str())),
        (
            "keys",
            Value::List(
                s.addresses
                    .iter()
                    .map(|a| Value::from(a.as_str()))
                    .collect(),
            ),
        ),
    ])
}

fn point(p: &Point) -> Value {
    Value::obj([("x", coord(p.x())), ("y", coord(p.y()))])
}

fn rect(r: &Rect) -> Value {
    Value::obj([
        ("x", coord(r.x())),
        ("y", coord(r.y())),
        ("w", coord(r.w())),
        ("h", coord(r.h())),
    ])
}

fn focus(f: &ElemFocus) -> Value {
    Value::obj([
        ("root", Value::from(f.root.as_str())),
        ("key", Value::from(f.address.as_str())),
    ])
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

/// The element presence fields of a roster entry or a device, each left out when `None`.
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

/// A time position: `{at, state, at_ms}`, then `in` and `rate` only when it has them.
fn time_position(t: &TimePosition) -> Value {
    let mut fields = vec![
        ("at", Value::from(t.at())),
        ("state", Value::from(t.state().as_str())),
        ("at_ms", Value::from(t.at_ms())),
    ];
    if let Some(a) = t.in_address() {
        fields.push(("in", Value::from(a)));
    }
    if let Some(r) = t.rate_text() {
        fields.push(("rate", Value::from(r)));
    }
    Value::obj(fields)
}

/// A roster entry's or a device's `view_address` and `time_position`, each left out when `None`.
fn view_time_fields(
    fields: &mut Vec<(&'static str, Value)>,
    view: &Option<String>,
    time: &Option<TimePosition>,
) {
    if let Some(v) = view {
        fields.push(("view_address", Value::from(v.as_str())));
    }
    if let Some(t) = time {
        fields.push(("time_position", time_position(t)));
    }
}

/// A presence roster event: `client` only when the event names one.
fn elem_event(
    kind: &'static str,
    w: &Who,
    value: Value,
    client: &Option<u64>,
    st: &mut Stable,
) -> Value {
    let mut fields = vec![("kind", Value::from(kind)), ("who", who(w)), (kind, value)];
    if let Some(c) = client {
        fields.push(("client", Value::from(st.client(*c))));
    }
    Value::obj(fields)
}

fn roster_event(e: &RosterEvent, st: &mut Stable) -> Value {
    match e {
        RosterEvent::Snapshot {
            version,
            members,
            watching,
            overwritten,
        } => {
            assert!(
                overwritten.is_empty(),
                "a snapshot with overwrite notes: this encoder doesn't carry them"
            );
            Value::obj([
                ("kind", Value::from("snapshot")),
                ("version", Value::from(*version)),
                (
                    "members",
                    Value::List(members.iter().map(|m| member(m, *version, st)).collect()),
                ),
                ("watching", Value::from(*watching)),
            ])
        }
        RosterEvent::Joined {
            who: w,
            participant: p,
            rights: r,
            ..
        } => Value::obj([
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
        RosterEvent::Left {
            who: w, reason: r, ..
        } => Value::obj([
            ("kind", Value::from("left")),
            ("who", who(w)),
            ("reason", Value::from(reason(*r))),
        ]),
        RosterEvent::Rights {
            who: w, rights: r, ..
        } => Value::obj([
            ("kind", Value::from("rights")),
            ("who", who(w)),
            ("rights", Value::from(rights(*r))),
        ]),
        RosterEvent::Role {
            who: w, role: r, ..
        } => Value::obj([
            ("kind", Value::from("role")),
            ("who", who(w)),
            ("role", Value::from(role(*r))),
        ]),
        RosterEvent::Away { who: w, away, .. } => Value::obj([
            ("kind", Value::from("away")),
            ("who", who(w)),
            ("away", Value::from(*away)),
        ]),
        RosterEvent::Cursor {
            who: w,
            cursor,
            version,
            client,
            ..
        } => {
            let mut fields = vec![
                ("kind", Value::from("cursor")),
                ("who", who(w)),
                (
                    "cursor",
                    cursor.as_ref().map_or(Value::Null, |c| caret(c, *version)),
                ),
                ("version", Value::from(*version)),
            ];
            // the connection whose caret this is (v0.10.1); absent: the participant's
            if let Some(c) = client {
                fields.push(("client", Value::from(st.client(*c))));
            }
            Value::obj(fields)
        }
        RosterEvent::Selection {
            who: w,
            selection: s,
            client,
            ..
        } => elem_event(
            "selection",
            w,
            s.as_ref().map_or(Value::Null, selection),
            client,
            st,
        ),
        RosterEvent::Focus {
            who: w,
            focus: f,
            client,
            ..
        } => elem_event(
            "focus",
            w,
            f.as_ref().map_or(Value::Null, focus),
            client,
            st,
        ),
        RosterEvent::Range {
            who: w,
            range: g,
            client,
            ..
        } => elem_event(
            "range",
            w,
            g.as_ref().map_or(Value::Null, range),
            client,
            st,
        ),
        RosterEvent::Pointer {
            who: w,
            pointer: p,
            client,
            ..
        } => elem_event(
            "pointer",
            w,
            p.as_ref().map_or(Value::Null, point),
            client,
            st,
        ),
        RosterEvent::Viewport {
            who: w,
            viewport: r,
            client,
            ..
        } => elem_event(
            "viewport",
            w,
            r.as_ref().map_or(Value::Null, rect),
            client,
            st,
        ),
        RosterEvent::ViewAddress {
            who: w,
            address,
            client,
            ..
        } => elem_event(
            "view_address",
            w,
            address.as_deref().map_or(Value::Null, Value::from),
            client,
            st,
        ),
        RosterEvent::TimePosition {
            who: w,
            time,
            client,
            ..
        } => elem_event(
            "time_position",
            w,
            time.as_ref().map_or(Value::Null, time_position),
            client,
            st,
        ),
        RosterEvent::ClaimStartedElems {
            who: w,
            claim,
            root,
            addresses,
            note,
            hold,
            expires_ms,
            version,
            actor: a,
            started_ms,
            ..
        } => Value::obj([
            ("kind", Value::from("claim_started_elems")),
            ("who", who(w)),
            ("claim", Value::from(st.claim(claim.get()))),
            ("root", Value::from(root.as_str())),
            ("keys", addresses_value(addresses)),
            ("note", note.as_deref().map_or(Value::Null, Value::from)),
            ("hold", Value::from(*hold)),
            ("expires_ms", Value::from(*expires_ms)),
            ("version", Value::from(*version)),
            ("actor", actor(w, a)),
            ("started_ms", Value::from(*started_ms)),
        ]),
        RosterEvent::ClaimEnded {
            who: w,
            claim,
            how,
            lines: l,
            version,
            actor: a,
            at_ms,
            at_rev,
            addresses,
            ..
        } => {
            let mut v = vec![
                ("kind", Value::from("claim_ended")),
                ("who", who(w)),
                ("claim", Value::from(st.claim(claim.get()))),
                ("how", Value::from(how.as_str())),
                ("lines", lines(l)),
                ("version", Value::from(*version)),
                ("actor", actor(w, a)),
                ("at_ms", Value::from(*at_ms)),
                ("at_rev", at_rev.map_or(Value::Null, Value::from)),
            ];
            // an element claim's elements by address, as they were: only when there are some
            if !addresses.is_empty() {
                v.push((
                    "keys",
                    Value::List(addresses.iter().map(|a| Value::from(a.as_str())).collect()),
                ));
            }
            Value::obj(v)
        }
        RosterEvent::Note {
            to,
            from,
            kind: k,
            lines: l,
            claim,
            ..
        } => Value::obj([
            ("kind", Value::from("note")),
            ("to", who(to)),
            ("from", who(from)),
            ("note", Value::from(k.as_str())),
            ("lines", lines(l)),
            (
                "claim",
                claim.map_or(Value::Null, |c| Value::from(st.claim(c.get()))),
            ),
        ]),
        RosterEvent::Watching { count, .. } => Value::obj([
            ("kind", Value::from("watching")),
            ("count", Value::from(*count)),
        ]),
        RosterEvent::Closed { reason: r } => {
            Value::obj([("kind", Value::from("closed")), ("reason", Value::from(*r))])
        }
        other => panic!("a roster event this turn's encoder doesn't encode: {other:?}"),
    }
}
