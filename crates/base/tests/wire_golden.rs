//! The live wire's JSON, pinned (client/WIRE.md, "JSON encodings"): golden files the JS client's
//! tests read and parse with `client/src/wire.ts`'s types (`client/test/golden.test.ts`).
//!
//! One session runs a small scenario through the public API: a person, an agent writing on
//! their behalf, a second person and an App viewer; live edits at a base (one carrying quotes,
//! a tab, a backslash, a two-byte and a four-byte character and a control character), an undo,
//! carets, away, a claim, a rights change and a leave; then the refusals the client acts on.
//! Every `ChangeEvent`, `RosterEvent` (with its `RosterMember`s) and `ErrorReport` it yields is
//! encoded as the wire carries it, by hand with the kit's own JSON writer
//! (`errors::Value`, as `ErrorReport::to_json`): the kit has no serde, and a host's encoder
//! must produce these same objects.
//!
//! Client IDs are random per run, so each is replaced by a fixed number (the person's by
//! `MAX_CLIENT_ID`, to pin that a 53-bit ID survives a JS number); claim ids are numbered in
//! the order they appear. Nothing else is rewritten.
//!
//! `MPC_BLESS=1` writes the files; otherwise they must match.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use k2_mpc_base::elem::ListOp;
use k2_mpc_base::errors::{lookup, ErrorReport, Value};
use k2_mpc_base::session::{
    At, AuditEvent, ChangeEvent, ClaimSpan, Config, ConnId, CursorAt, EditOptions, ElemSelection,
    Host, JoinOptions, LeaveReason, LineRange, Overwrite, Participant, ParticipantKind, Point,
    Rect, Rights, Role, RosterEvent, RosterMember, Session, SessionManager, Stay, TextFilePack,
    Who, OVERWRITE_PENDING_MAX,
};
use k2_mpc_base::store::{DocKey, DocStore, Seq};
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::MAX_CLIENT_ID;
use k2_mpc_base::{Edit, Key};

#[path = "keyed_pack/mod.rs"]
mod keyed_pack;

const ROOT: &str = TextFilePack::ROOT;
const DOC: &str = "# Notes\n\none\ntwo\nthree\n";
/// The agent's line: JSON escapes (quote, tab, backslash, U+0001) and multi-byte characters.
const TRICKY: &str = "say \"hi\"\t\\ caf\u{e9} \u{1F600}\u{1}\n";

struct TestHost {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        self.rights
            .lock()
            .unwrap()
            .get(&who.pid)
            .copied()
            .unwrap_or(Rights::editor())
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct TmpDir(PathBuf);

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn golden_dir() -> PathBuf {
    manifest().join("../../client/test/golden")
}

fn golden(path: &Path, got: &str) {
    if std::env::var_os("MPC_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(path)
        .unwrap_or_else(|_| panic!("{} is missing (MPC_BLESS=1 writes it)", path.display()));
    assert!(
        want == got,
        "{} differs from what the kit encodes now (MPC_BLESS=1 regenerates it)\n--- now:\n{got}",
        path.display()
    );
}

fn drain<T>(rx: &Receiver<T>) -> Vec<T> {
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

// ---------- the encodings (WIRE.md, "JSON encodings") ----------

/// Fixed numbers for the run's random client IDs, and claim ids in order of appearance.
#[derive(Default)]
struct Stable {
    clients: HashMap<u64, u64>,
    claims: HashMap<u64, u64>,
}

impl Stable {
    fn client(&self, c: u64) -> u64 {
        *self
            .clients
            .get(&c)
            .unwrap_or_else(|| panic!("client {c} isn't one of the scenario's"))
    }
    fn claim(&mut self, id: u64) -> String {
        let n = self.claims.len() as u64 + 1;
        format!("claim-{}", self.claims.entry(id).or_insert(n))
    }
}

fn hunk(h: &Hunk) -> Value {
    Value::obj([
        ("start", Value::from(h.start)),
        ("end", Value::from(h.end)),
        ("insert", Value::from(h.insert.as_str())),
    ])
}

fn change(e: &ChangeEvent, st: &Stable) -> Value {
    Value::obj([
        ("version", Value::from(e.version)),
        ("pid", Value::from(e.pid.as_str())),
        ("client", Value::from(st.client(e.client))),
        ("via", Value::from(e.via.as_str())),
        ("kind", Value::from(e.kind.as_str())),
        ("root", Value::from(e.root.as_str())),
        ("hunks", Value::List(e.hunks.iter().map(hunk).collect())),
        ("at_ms", Value::from(e.at_ms)),
        (
            "on_behalf_of",
            e.on_behalf_of.as_deref().map_or(Value::Null, Value::from),
        ),
    ])
}

/// `edit` when the participant may edit, else `read` when they may read, else `none`.
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

/// `RosterMember` → the wire's `RosterEntry`. `devices` (v0.10.1) only when the kit gives
/// some (a text doc): one `{client, cursor, label?}` a connection, `label` only when set.
/// `elem_claims` only when the member holds claims on elements (`ClaimSpan::Elems`), so a late
/// subscriber sees who holds what.
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
    );
    if let Some(Value::List(devices)) = fields
        .iter_mut()
        .find(|(k, _)| *k == "devices")
        .map(|(_, v)| v)
    {
        for (o, d) in devices.iter_mut().zip(&m.entry.devices) {
            let Value::Obj(o) = o else { continue };
            let mut more = Vec::new();
            elem_fields(&mut more, &d.selection, &d.pointer, &d.viewport);
            o.extend(more.into_iter().map(|(k, v)| (k.to_owned(), v)));
        }
    }
    let elem_claims: Vec<Value> = m
        .entry
        .claims
        .iter()
        .filter_map(|c| match &c.span {
            ClaimSpan::Elems { root, keys } => Some(Value::obj([
                ("claim", Value::from(st.claim(c.id.get()))),
                ("root", Value::from(root.as_str())),
                ("keys", keys_value(keys)),
                ("note", c.note.as_deref().map_or(Value::Null, Value::from)),
                ("hold", Value::from(c.hold)),
                ("expires_ms", Value::from(c.expires_ms)),
            ])),
            _ => None,
        })
        .collect();
    if !elem_claims.is_empty() {
        fields.push(("elem_claims", Value::List(elem_claims)));
    }
    Value::obj(fields)
}

fn keys_value(keys: &[k2_mpc_base::Key]) -> Value {
    Value::List(keys.iter().map(|k| Value::from(k.as_str())).collect())
}

/// A coordinate: the scenarios use whole numbers, which `errors::Value` encodes as they are.
fn coord(v: f64) -> Value {
    assert_eq!(v.fract(), 0.0, "the golden scenarios use whole numbers");
    Value::Int(v as i64)
}

fn selection(s: &ElemSelection) -> Value {
    Value::obj([
        ("root", Value::from(s.root.as_str())),
        (
            "keys",
            Value::List(s.keys.iter().map(|k| Value::from(k.as_str())).collect()),
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

/// The element presence fields of a roster entry or a device, each left out when `None`.
fn elem_fields(
    fields: &mut Vec<(&'static str, Value)>,
    s: &Option<ElemSelection>,
    p: &Option<Point>,
    r: &Option<Rect>,
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
}

/// A `selection`, `pointer` or `viewport` roster event: `client` only when the event names one.
fn elem_event(
    kind: &'static str,
    w: &Who,
    value: Value,
    client: &Option<u64>,
    st: &Stable,
) -> Value {
    let mut fields = vec![("kind", Value::from(kind)), ("who", who(w)), (kind, value)];
    if let Some(c) = client {
        fields.push(("client", Value::from(st.client(*c))));
    }
    Value::obj(fields)
}

/// One place of an `Overwrite` → a cell of the wire's `cells`.
fn cell(region: &str, col: Option<usize>, yours: &str, file: &str, cut: bool) -> Value {
    Value::obj([
        ("region", Value::from(region)),
        ("col", col.map_or(Value::Null, Value::from)),
        ("yours", Value::from(yours)),
        ("file", Value::from(file)),
        ("cut", Value::from(cut)),
    ])
}

/// `Overwrite` → the wire's `overwritten` event (WIRE.md, "The `overwritten` event"), to `to`:
/// its first place and `also` are `cells` (none for a roll-up), `message` is its `Display`.
fn overwrite(to: Value, o: &Overwrite) -> Value {
    let cells: Vec<Value> = if o.is_rollup() {
        Vec::new()
    } else {
        std::iter::once(cell(&o.region, o.col, &o.yours, &o.file, o.cut))
            .chain(
                o.also
                    .iter()
                    .map(|c| cell(&c.region, c.col, &c.yours, &c.file, c.cut)),
            )
            .collect()
    };
    Value::obj([
        ("kind", Value::from("overwritten")),
        ("to", to),
        ("change", Value::from(o.change)),
        ("version", o.version.map_or(Value::Null, Value::from)),
        (
            "restore_from",
            o.restore_from.map_or(Value::Null, Value::from),
        ),
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

/// `me`: the subscriber's own `who` (its waiting notes in a snapshot are to it).
fn roster_event(e: &RosterEvent, st: &mut Stable, me: &Value) -> Value {
    match e {
        RosterEvent::Snapshot {
            version,
            members,
            watching,
            overwritten,
        } => {
            let mut fields = vec![
                ("kind", Value::from("snapshot")),
                ("version", Value::from(*version)),
                (
                    "members",
                    Value::List(members.iter().map(|m| member(m, *version, st)).collect()),
                ),
                ("watching", Value::from(*watching)),
            ];
            // present only when this subscriber has waiting notes (WIRE.md)
            if !overwritten.is_empty() {
                fields.push((
                    "overwritten",
                    Value::List(
                        overwritten
                            .iter()
                            .map(|o| overwrite(me.clone(), o))
                            .collect(),
                    ),
                ));
            }
            Value::obj(fields)
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
        RosterEvent::ClaimStarted {
            who: w,
            claim,
            lines: l,
            note,
            hold,
            for_edit,
            expires_ms,
            version,
            ..
        } => Value::obj([
            ("kind", Value::from("claim_started")),
            ("who", who(w)),
            ("claim", Value::from(st.claim(claim.get()))),
            ("lines", lines(l)),
            ("note", note.as_deref().map_or(Value::Null, Value::from)),
            ("hold", Value::from(*hold)),
            ("for_edit", Value::from(*for_edit)),
            ("expires_ms", Value::from(*expires_ms)),
            ("version", Value::from(*version)),
        ]),
        RosterEvent::ClaimStartedElems {
            who: w,
            claim,
            root,
            keys,
            note,
            hold,
            expires_ms,
            version,
            ..
        } => Value::obj([
            ("kind", Value::from("claim_started_elems")),
            ("who", who(w)),
            ("claim", Value::from(st.claim(claim.get()))),
            ("root", Value::from(root.as_str())),
            ("keys", keys_value(keys)),
            ("note", note.as_deref().map_or(Value::Null, Value::from)),
            ("hold", Value::from(*hold)),
            ("expires_ms", Value::from(*expires_ms)),
            ("version", Value::from(*version)),
        ]),
        RosterEvent::ClaimEnded {
            who: w,
            claim,
            how,
            lines: l,
            version,
            ..
        } => Value::obj([
            ("kind", Value::from("claim_ended")),
            ("who", who(w)),
            ("claim", Value::from(st.claim(claim.get()))),
            ("how", Value::from(how.as_str())),
            ("lines", lines(l)),
            ("version", Value::from(*version)),
        ]),
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
        RosterEvent::Overwritten { to, overwrite: o } => overwrite(who(to), o),
        RosterEvent::Closed { reason: r } => {
            Value::obj([("kind", Value::from("closed")), ("reason", Value::from(*r))])
        }
        other => panic!("a roster event this test doesn't encode yet: {other:?}"),
    }
}

fn file(v: Value) -> String {
    let mut s = v.to_json_pretty();
    s.push('\n');
    s
}

// ---------- the scenario ----------

struct Fx {
    _dir: TmpDir,
    host: Arc<TestHost>,
    mgr: SessionManager,
    s: Arc<Session>,
}

fn fx() -> Fx {
    let dir = std::env::temp_dir().join(format!("k2mb-wire-golden-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_700_000_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    let mut config = Config::default();
    // a small live window, so `events_since` refuses (MPC-E112) after a few changes
    config.live_window_events = 2;
    config.edit_rate_per_s = 1_000_000;
    config.edit_burst = 1_000_000;
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, host.clone(), config);
    let s = mgr
        .open(
            Arc::new(TextFilePack::new("markdown", 4 << 20)),
            &DocKey {
                space: "ws".into(),
                path: "notes.md".into(),
            },
            Some(DOC.as_bytes()),
        )
        .unwrap();
    Fx {
        _dir: TmpDir(dir),
        host,
        mgr,
        s,
    }
}

impl Fx {
    fn tick(&self) {
        self.host.now.fetch_add(300, Ordering::SeqCst);
        self.mgr.flush_rosters();
    }
    fn version(&self) -> Seq {
        self.s.status().unwrap().version
    }
    fn text(&self, c: ConnId) -> String {
        self.s
            .read(c)
            .unwrap()
            .text_by_root
            .into_iter()
            .find(|(r, _)| r == ROOT)
            .unwrap()
            .1
    }
    fn edit(&self, c: ConnId, base: Seq, hunks: Vec<Hunk>) -> Result<Vec<Hunk>, ErrorReport> {
        self.host.now.fetch_add(7, Ordering::SeqCst);
        self.s
            .edit_at_base(c, ROOT, base, hunks, &EditOptions::new())
            .map(|r| r.hunks)
    }
}

fn h(start: usize, end: usize, insert: &str) -> Hunk {
    Hunk {
        start,
        end,
        insert: insert.into(),
    }
}

#[test]
fn the_wire_json_is_golden() {
    let f = fx();
    let join = |p: Participant| f.s.join(p, Stay::Held).unwrap();
    let ada = join(Participant::new(
        "ada",
        "Ada",
        "#7c3aed",
        ParticipantKind::Person,
        "tab",
    ));
    let agent = join(
        Participant::new(
            "agent-7",
            "Helper",
            "#0ea5e9",
            ParticipantKind::Agent,
            "cli",
        )
        .on_behalf_of("ada"),
    );
    let app = join(Participant::new(
        "widget",
        "Widget",
        "#64748b",
        ParticipantKind::App,
        "app",
    ));
    let mut st = Stable::default();
    st.clients.insert(ada.client, MAX_CLIENT_ID);
    st.clients.insert(agent.client, 2);
    st.clients.insert(app.client, 3);
    let (ada, agent, app) = (ada.conn, agent.conn, app.conn);

    // subscribe first, then read (WIRE.md, "Connecting")
    let changes = f.s.subscribe(ada).unwrap();
    let changes_app = f.s.subscribe(app).unwrap();
    let roster = f.s.subscribe_roster(ada).unwrap();
    let roster_app = f.s.subscribe_roster(app).unwrap();
    let v0 = f.version();
    let text0 = f.text(ada);
    assert_eq!(text0, DOC);

    // a second person joins (a roster delta), after the snapshot
    let bob = join(Participant::new(
        "bob",
        "Bob",
        "#16a34a",
        ParticipantKind::Person,
        "tab",
    ));
    st.clients.insert(bob.client, 4);
    let bob = bob.conn;
    f.tick();

    // ada types a line at a base; the agent edits at the same (now stale) base
    let mut errors: Vec<(&str, ErrorReport)> = Vec::new();
    f.edit(ada, v0, vec![h(9, 9, "zero\n")]).unwrap();
    let applied = f
        .edit(agent, v0, vec![h(13, 17, ""), h(23, 23, TRICKY)])
        .unwrap();
    assert_eq!(applied, vec![h(18, 22, ""), h(28, 28, TRICKY)]);
    // ada replaces "two", which the agent deleted since her base: a write conflict
    let e = f.edit(ada, v0, vec![h(13, 16, "TWO")]).unwrap_err();
    assert_eq!(e.id(), "MPC-E111");
    errors.push(("write_conflict", e));
    // a hunk past the end: refused before anything else
    let v = f.version();
    let e = f.edit(ada, v, vec![h(10_000, 10_000, "x")]).unwrap_err();
    assert_eq!(e.id(), "MPC-E151");
    errors.push(("bad_hunk", e));
    // ada undoes her line: it comes back as a change with her own pid and client ID
    let undone = f.s.undo(ada, 1).unwrap();
    assert!(format!("{undone:?}").starts_with("Applied"), "{undone:?}");
    let v_end = f.version();
    // past the live window (2 changes here): MPC-E112
    let e = f.s.events_since(ada, v0).unwrap_err();
    assert_eq!(e.id(), "MPC-E112");
    errors.push(("base_expired", e));
    let since = f.s.events_since(ada, v_end - 2).unwrap();
    assert_eq!(since.len(), 2);

    // carets: a selection, and a caret after the four-byte character (line:col counts
    // characters, bytes count bytes)
    let text = f.text(ada);
    let emoji = text.find('\u{1F600}').unwrap() + 4;
    f.s.set_cursor(ada, ROOT, At::Byte(0), At::Byte(7)).unwrap();
    f.s.set_cursor(bob, ROOT, At::Byte(emoji), At::Byte(emoji))
        .unwrap();
    // a caret off a character boundary: MPC-E114 (the host ignores it: `cursor` has no reply)
    let e =
        f.s.set_cursor(bob, ROOT, At::Byte(emoji - 1), At::Byte(emoji - 1))
            .unwrap_err()
            .report();
    assert_eq!(e.id(), "MPC-E114");
    errors.push(("bad_position", e));
    f.tick();

    // bob's tab goes to the back; past away_after_ms he shows away
    f.s.set_visible(bob, false).unwrap();
    f.host.now.fetch_add(60_001, Ordering::SeqCst);
    f.tick();

    // the agent claims the heading and releases it
    let claim =
        f.s.claim(
            agent,
            LineRange {
                root: ROOT.into(),
                first: 1,
                last: 2,
            },
            Some(30_000),
            Some("tidying the heading".into()),
            false,
        )
        .unwrap();
    f.tick();
    f.s.release_claim(agent, Some(claim.id)).unwrap();
    f.tick();

    // bob drops to read-only, then leaves
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("bob".into(), Rights::read_only());
    f.s.recheck_rights().unwrap();
    f.tick();
    f.s.leave(bob).unwrap();
    f.tick();

    // the reports the host builds itself (no session call returns them)
    let reg = |id: &str, msg: &str| ErrorReport::new(lookup(id).unwrap(), msg);
    errors.push((
        "bad_edit_frame_cap",
        reg(
            "MPC-E113",
            "bad edit: the frame is 4194400 bytes, over the 4194304-byte cap",
        )
        .with("bytes", 4_194_400u64)
        .with("frame_max", 4_194_304u64),
    ));
    errors.push((
        "declined_too_large",
        reg(
            "MPC-E401",
            "live editing is off: the text doesn't fit one frame (too_large)",
        )
        .with("limit", "frame")
        .with("bytes", 6_291_502u64)
        .with("frame_max", 4_194_304u64),
    ));
    errors.push((
        "history_lost",
        reg(
            "MPC-E206",
            "nothing to undo: your undo history was lost when the doc reloaded",
        ),
    ));

    // ---------- the files ----------
    let dir = golden_dir();
    let evs = drain(&changes);
    assert_eq!(evs.len(), 3, "ada's edit, the agent's, the undo: {evs:?}");
    let final_text = f.text(ada);
    golden(
        &dir.join("change_events.json"),
        &file(Value::obj([
            ("root", Value::from(ROOT)),
            ("version", Value::from(v0)),
            ("text", Value::from(text0.as_str())),
            (
                "events",
                Value::List(evs.iter().map(|e| change(e, &st)).collect()),
            ),
            ("final_version", Value::from(v_end)),
            ("final_text", Value::from(final_text.as_str())),
        ])),
    );
    let evs_app = drain(&changes_app);
    assert_eq!(evs_app.len(), 3);
    assert!(
        evs_app
            .iter()
            .all(|e| !e.pid.contains("ada") && !e.pid.contains("agent")),
        "an App viewer sees handles, never pids: {evs_app:?}"
    );
    golden(
        &dir.join("change_events_app.json"),
        &file(Value::obj([
            ("root", Value::from(ROOT)),
            ("version", Value::from(v0)),
            ("text", Value::from(text0.as_str())),
            (
                "events",
                Value::List(evs_app.iter().map(|e| change(e, &st)).collect()),
            ),
            ("final_version", Value::from(v_end)),
            ("final_text", Value::from(final_text.as_str())),
        ])),
    );

    // the roster, as each lens saw it, and the kit's own roster at the end (a fresh
    // subscription's snapshot): folding the events must give the same members
    let ros = drain(&roster);
    let ros_app = drain(&roster_app);
    let end = drain(&f.s.subscribe_roster(ada).unwrap());
    let end_app = drain(&f.s.subscribe_roster(app).unwrap());
    let mut kinds: Vec<&str> = Vec::new();
    for e in &ros {
        let k = match e {
            RosterEvent::Snapshot { .. } => "snapshot",
            RosterEvent::Joined { .. } => "joined",
            RosterEvent::Left { .. } => "left",
            RosterEvent::Rights { .. } => "rights",
            RosterEvent::Role { .. } => "role",
            RosterEvent::Away { .. } => "away",
            RosterEvent::Cursor { .. } => "cursor",
            RosterEvent::ClaimStarted { .. } => "claim_started",
            RosterEvent::ClaimStartedElems { .. } => "claim_started_elems",
            RosterEvent::ClaimEnded { .. } => "claim_ended",
            RosterEvent::Note { .. } => "note",
            RosterEvent::Watching { .. } => "watching",
            _ => "other",
        };
        if !kinds.contains(&k) {
            kinds.push(k);
        }
    }
    for k in [
        "snapshot",
        "joined",
        "left",
        "rights",
        "role",
        "away",
        "cursor",
        "claim_started",
        "claim_ended",
    ] {
        assert!(kinds.contains(&k), "no {k} event in {ros:?}");
    }
    let lens = |name: &str, evs: &[RosterEvent], end: &[RosterEvent], st: &mut Stable| {
        // the connection's own `who`, as `welcome.self` gives it: a pid, or for an App
        // viewer its own handle (its member row)
        let me = if name == "ada" { "Ada" } else { "Widget" };
        let self_who = match &end[0] {
            RosterEvent::Snapshot { members, .. } => members
                .iter()
                .find(|m| m.entry.participant.name == me)
                .map_or(Value::Null, |m| who(&m.who)),
            other => panic!("{other:?}"),
        };
        let events: Vec<Value> = evs.iter().map(|e| roster_event(e, st, &self_who)).collect();
        let fin = roster_event(&end[0], st, &self_who);
        Value::obj([
            ("self", self_who),
            ("events", Value::List(events)),
            ("final", fin),
        ])
    };
    let ada_lens = lens("ada", &ros, &end, &mut st);
    let app_lens = lens("app", &ros_app, &end_app, &mut st);
    golden(&dir.join("roster_events.json"), &file(ada_lens));
    golden(&dir.join("roster_events_app.json"), &file(app_lens));

    golden(
        &dir.join("errors.json"),
        &file(Value::obj(
            errors
                .iter()
                .map(|(name, r)| (name.to_string(), r.to_value()))
                .collect::<Vec<_>>(),
        )),
    );
    // the wire's bytes for a report are `to_json` itself: the golden object is its value
    for (_, r) in &errors {
        assert_eq!(r.to_json(), r.to_value().to_json());
    }
}

// ---------- outside saves, a roll-up and the host's close (a session on a file) ----------

/// A session opened on a real file, so an outside program can save it.
struct FileFx {
    _dir: TmpDir,
    file: PathBuf,
    host: Arc<TestHost>,
    mgr: SessionManager,
    key: DocKey,
    s: Arc<Session>,
    /// Each joined connection's client ID → its fixed number ([`fixed_client`]).
    clients: Mutex<HashMap<u64, u64>>,
}

/// The fixed number for a participant's client ID in the file scenarios: the main
/// scenario's numbers for the same pids.
fn fixed_client(pid: &str) -> u64 {
    match pid {
        "ada" => MAX_CLIENT_ID,
        "agent-7" => 2,
        "widget" => 3,
        "bob" => 4,
        other => panic!("no fixed client ID for {other}"),
    }
}

fn file_fx(tag: &str) -> FileFx {
    let dir = std::env::temp_dir().join(format!("k2mb-wire-golden-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let file = dir.join("ws/notes.md");
    std::fs::write(&file, DOC).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_700_000_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    let mut config = Config::default();
    config.edit_rate_per_s = 1_000_000;
    config.edit_burst = 1_000_000;
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, host.clone(), config);
    let key = DocKey {
        space: "ws".into(),
        path: "notes.md".into(),
    };
    let s = mgr
        .open_file(
            Arc::new(TextFilePack::new("markdown", 4 << 20)),
            &key,
            &file,
            &dir.join("ws"),
        )
        .unwrap();
    FileFx {
        _dir: TmpDir(dir),
        file,
        host,
        mgr,
        key,
        s,
        clients: Mutex::new(HashMap::new()),
    }
}

impl FileFx {
    fn tick(&self) {
        self.host.now.fetch_add(300, Ordering::SeqCst);
        self.mgr.flush_rosters();
    }
    fn join(
        &self,
        pid: &str,
        name: &str,
        colour: &str,
        kind: ParticipantKind,
        via: &str,
    ) -> ConnId {
        let j = self
            .s
            .join(Participant::new(pid, name, colour, kind, via), Stay::Held)
            .unwrap();
        self.clients
            .lock()
            .unwrap()
            .insert(j.client, fixed_client(pid));
        j.conn
    }
    /// The run's client IDs, fixed (the main scenario's rewrite).
    fn stable(&self) -> Stable {
        Stable {
            clients: self.clients.lock().unwrap().clone(),
            ..Stable::default()
        }
    }
    fn text(&self, c: ConnId) -> String {
        self.s
            .read(c)
            .unwrap()
            .text_by_root
            .into_iter()
            .find(|(r, _)| r == ROOT)
            .unwrap()
            .1
    }
    /// `c` types `text` at byte `at`, acknowledged (an ordinary edit, not a burst).
    fn type_at(&self, c: ConnId, at: usize, text: &str) {
        self.host.now.fetch_add(1_000, Ordering::SeqCst);
        let base = self.s.status().unwrap().version;
        self.s
            .edit_at_base(c, ROOT, base, vec![h(at, at, text)], &EditOptions::new())
            .unwrap();
    }
    /// The kit writes the doc back, then an outside program saves `text`.
    fn save(&self, c: ConnId, text: &str) {
        self.s.flush(c).unwrap();
        self.host.now.fetch_add(10_000, Ordering::SeqCst);
        std::fs::write(&self.file, text).unwrap();
        self.s.file_changed().unwrap();
    }
}

/// Every event until the subscription ends; it must end (the sender dropped), not just pause.
fn drain_to_end(rx: &Receiver<RosterEvent>) -> Vec<RosterEvent> {
    let mut v = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(e) => v.push(e),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => return v,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                panic!("the subscription didn't end after `closed`: {v:?}")
            }
        }
    }
}

fn pushes(evs: &[RosterEvent]) -> Vec<(&Who, &Overwrite)> {
    evs.iter()
        .filter_map(|e| match e {
            RosterEvent::Overwritten { to, overwrite } => Some((to, overwrite)),
            _ => None,
        })
        .collect()
}

fn waiting(snap: &RosterEvent) -> &[Overwrite] {
    match snap {
        RosterEvent::Snapshot { overwritten, .. } => overwritten,
        other => panic!("not a snapshot: {other:?}"),
    }
}

/// The subscriber's own `who` in a snapshot, by its participant's name.
fn self_in(snap: &RosterEvent, name: &str) -> Value {
    match snap {
        RosterEvent::Snapshot { members, .. } => members
            .iter()
            .find(|m| m.entry.participant.name == name)
            .map_or(Value::Null, |m| who(&m.who)),
        other => panic!("not a snapshot: {other:?}"),
    }
}

/// Ada (a person who may read history) types two lines and an App viewer one; an outside save
/// removes all three. Ada types another line; a second save replaces it. Each author's own
/// roster subscription gets an `overwritten` push live (Bob, another person, gets none); a
/// later subscription finds the same notes waiting in its snapshot. Then the host closes the
/// session: `closed` is every subscription's last event, and they end.
///
/// `roster_events_overwritten{,_app}.json`: each lens's first subscription, its whole stream
/// through `closed`, and `final`, a later subscription's snapshot (its waiting notes in
/// `overwritten`). `roster_events_closed{,_app}.json`: that later subscription's whole stream,
/// its snapshot then `closed`.
#[test]
fn outside_saves_and_the_hosts_close_are_golden() {
    let f = file_fx("overwritten");
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("ada".into(), Rights::full());
    let ada = f.join("ada", "Ada", "#7c3aed", ParticipantKind::Person, "tab");
    let bob = f.join("bob", "Bob", "#16a34a", ParticipantKind::Person, "tab");
    let app = f.join("widget", "Widget", "#64748b", ParticipantKind::App, "app");
    let roster = f.s.subscribe_roster(ada).unwrap();
    let roster_app = f.s.subscribe_roster(app).unwrap();
    let roster_bob = f.s.subscribe_roster(bob).unwrap();
    f.tick();

    // save 1: removes Ada's two lines (two places) and the App's line
    f.type_at(ada, "# Notes\n\n".len(), "first by ada\n");
    let three = f.text(ada).find("three").unwrap();
    f.type_at(ada, three, "second by ada\n");
    let end = f.text(app).len();
    f.type_at(app, end, "by the widget\n");
    f.save(ada, DOC);
    assert_eq!(f.text(ada), DOC, "the outside save is the text");
    f.tick();

    // save 2: replaces Ada's next line
    f.type_at(ada, "# Notes\n\n".len(), "ada's draft\n");
    let typed = f.text(ada);
    f.save(ada, &typed.replace("ada's draft", "the editor's draft"));
    f.tick();

    // later subscriptions: the notes wait in their snapshots (a push doesn't consume them)
    let late = f.s.subscribe_roster(ada).unwrap();
    let late_app = f.s.subscribe_roster(app).unwrap();
    let late_bob = f.s.subscribe_roster(bob).unwrap();

    // the host closes the doc's live session
    assert!(f.mgr.close(&f.key, "core_off").unwrap());
    let ros = drain_to_end(&roster);
    let ros_app = drain_to_end(&roster_app);
    let ros_bob = drain_to_end(&roster_bob);
    let late = drain_to_end(&late);
    let late_app = drain_to_end(&late_app);
    let late_bob = drain_to_end(&late_bob);
    for evs in [&ros, &ros_app, &ros_bob, &late, &late_app, &late_bob] {
        assert!(
            matches!(evs.last(), Some(RosterEvent::Closed { reason: "core_off" })),
            "`closed` is the last event: {evs:?}"
        );
        assert_eq!(
            evs.iter()
                .filter(|e| matches!(e, RosterEvent::Closed { .. }))
                .count(),
            1
        );
    }

    // the pushes: to their authors only
    let to_ada = pushes(&ros);
    assert_eq!(to_ada.len(), 2, "{ros:?}");
    let (o1, o2) = (to_ada[0].1, to_ada[1].1);
    assert_eq!(o1.rule, "removed_text");
    assert_eq!(o1.yours, "first by ada\n");
    assert_eq!(o1.also.len(), 1, "two places, one note");
    assert_eq!(o1.also[0].yours, "second by ada\n");
    assert!(o1.version.is_some() && !o1.no_history, "{o1:?}");
    assert_eq!(o2.rule, "replaced_text");
    assert_eq!(o2.yours, "ada's draft\n");
    assert_eq!(o2.file, "the editor's draft");
    let to_app = pushes(&ros_app);
    assert_eq!(to_app.len(), 1, "{ros_app:?}");
    assert_eq!(to_app[0].1.yours, "by the widget\n");
    assert!(
        to_app[0].1.version.is_none() && to_app[0].1.no_history,
        "no history right: no version to open"
    );
    assert!(
        pushes(&ros_bob).is_empty(),
        "only the author's subscriptions"
    );
    // the same notes wait in the later snapshots; dedupe on (change, row)
    let key = |o: &Overwrite| (o.change, o.row.clone());
    assert_eq!(
        waiting(&late[0]).iter().map(key).collect::<Vec<_>>(),
        to_ada.iter().map(|(_, o)| key(o)).collect::<Vec<_>>()
    );
    assert_eq!(
        waiting(&late_app[0]).iter().map(key).collect::<Vec<_>>(),
        to_app.iter().map(|(_, o)| key(o)).collect::<Vec<_>>()
    );
    assert!(waiting(&late_bob[0]).is_empty());
    assert_eq!(late_bob.len(), 2, "a snapshot, then `closed`: {late_bob:?}");

    let mut st = f.stable();
    let dir = golden_dir();
    for (lens, name, evs, later) in [
        ("", "Ada", &ros, &late),
        ("_app", "Widget", &ros_app, &late_app),
    ] {
        let me = self_in(&later[0], name);
        golden(
            &dir.join(format!("roster_events_overwritten{lens}.json")),
            &file(Value::obj([
                ("self", me.clone()),
                (
                    "events",
                    Value::List(evs.iter().map(|e| roster_event(e, &mut st, &me)).collect()),
                ),
                ("final", roster_event(&later[0], &mut st, &me)),
            ])),
        );
        golden(
            &dir.join(format!("roster_events_closed{lens}.json")),
            &file(Value::obj([
                ("self", me.clone()),
                (
                    "events",
                    Value::List(
                        later
                            .iter()
                            .map(|e| roster_event(e, &mut st, &me))
                            .collect(),
                    ),
                ),
            ])),
        );
    }
}

/// One person's notes past the cap: `OVERWRITE_PENDING_MAX + 2` saves, each removing one of
/// Ada's lines, nothing delivered in between. Every save pushes; the 64th waiting note becomes
/// a roll-up from the 65th save on, and counts the rest in `more`.
///
/// `roster_events_rollup.json` keeps the file reviewable: `pushed` counts the live pushes and
/// `tail` holds the last three as they came (the 64th note, then the roll-up twice); `final`
/// is a later subscription's snapshot, all 64 waiting notes, the last the roll-up. Nothing in
/// the events is rewritten; only the live stream is cut to its tail.
#[test]
fn the_roll_up_is_golden() {
    let n = OVERWRITE_PENDING_MAX + 2;
    let f = file_fx("rollup");
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("ada".into(), Rights::full());
    let ada = f.join("ada", "Ada", "#7c3aed", ParticipantKind::Person, "tab");
    let roster = f.s.subscribe_roster(ada).unwrap();
    f.tick();
    let body: String = (0..n).map(|i| format!("line {i}\n")).collect();
    f.type_at(ada, "# Notes\n\n".len(), &body);
    let mut text = f.text(ada);
    for i in 0..n {
        text = text.replace(&format!("line {i}\n"), "");
        f.save(ada, &text);
        f.tick();
    }
    assert_eq!(f.text(ada), DOC);
    let late = drain(&f.s.subscribe_roster(ada).unwrap());
    let ros = drain(&roster);

    let pushed = pushes(&ros);
    assert_eq!(pushed.len(), n, "every save pushes");
    let notes = waiting(&late[0]);
    assert_eq!(notes.len(), OVERWRITE_PENDING_MAX);
    let last = notes.last().unwrap();
    assert!(last.is_rollup(), "{last:?}");
    assert!(notes[..notes.len() - 1].iter().all(|o| !o.is_rollup()));
    assert_eq!(last.more, 3, "the 64th note and the two after it");
    assert_eq!(
        notes.iter().map(Overwrite::cells).sum::<usize>(),
        n,
        "a note is never dropped silently"
    );
    let tail: Vec<&RosterEvent> = ros
        .iter()
        .filter(|e| matches!(e, RosterEvent::Overwritten { .. }))
        .skip(n - 3)
        .collect();
    assert!(
        matches!(tail[1], RosterEvent::Overwritten { overwrite, .. } if overwrite.is_rollup() && overwrite.more == 2)
    );

    let mut st = f.stable();
    let me = self_in(&late[0], "Ada");
    golden(
        &golden_dir().join("roster_events_rollup.json"),
        &file(Value::obj([
            (
                "about",
                Value::from(
                    "One person's notes past the cap of 64: `saves` outside saves each removed one of \
                     their lines. `pushed` counts the live `overwritten` pushes; `tail` is the last \
                     three as they came (the 64th note, then the roll-up twice); `final` is a later \
                     subscription's snapshot with all 64 waiting notes, the last the roll-up.",
                ),
            ),
            ("self", me.clone()),
            ("saves", Value::from(n)),
            ("pushed", Value::from(pushed.len())),
            (
                "tail",
                Value::List(
                    tail.iter()
                        .map(|e| roster_event(e, &mut st, &me))
                        .collect(),
                ),
            ),
            ("final", roster_event(&late[0], &mut st, &me)),
        ])),
    );
}

// ---------- per-connection carets (v0.10.1) ----------

/// The same person on three connections (two of them labelled by the host) and a second
/// person. Every connection's caret is its own `cursor` event (`client`), and each member's
/// `devices` lists its connections with their carets. Ada's first connection subscribes: it
/// is the file's `client`, the connection a client hides. Carets on all three, a line typed
/// by Bob above them (each carried, one event a connection, the most active last), then the
/// most active connection leaves with its caret: its `cursor` goes (`null`), and a `cursor`
/// with no `client` gives Ada's caret now, for a client that keeps one caret a person.
///
/// `roster_events_devices.json`: `self`, `client`, the stream, and `final`, a later
/// subscription's snapshot.
#[test]
fn per_connection_carets_are_golden() {
    let f = file_fx("devices");
    let ada = |opts: JoinOptions| {
        f.s.join_with(
            Participant::new("ada", "Ada", "#7c3aed", ParticipantKind::Person, "tab"),
            Stay::Held,
            &opts,
        )
        .unwrap()
    };
    let t1 = ada(JoinOptions::new());
    let bob = f.join("bob", "Bob", "#16a34a", ParticipantKind::Person, "tab");
    let mut st = f.stable();
    st.clients.insert(t1.client, MAX_CLIENT_ID);
    let roster = f.s.subscribe_roster(t1.conn).unwrap();
    let t2 = ada(JoinOptions::new().label("another tab"));
    let t3 = ada(JoinOptions::new().label("Phone"));
    st.clients.insert(t2.client, 5);
    st.clients.insert(t3.client, 6);
    f.tick();

    let at = |line: usize| f.text(t1.conn).match_indices('\n').nth(line - 2).unwrap().0 + 1;
    f.s.set_cursor(t1.conn, ROOT, At::Byte(0), At::Byte(0))
        .unwrap();
    f.host.now.fetch_add(10, Ordering::SeqCst);
    f.s.set_cursor(t2.conn, ROOT, At::Byte(at(3)), At::Byte(at(3)))
        .unwrap();
    f.tick();
    f.s.set_cursor(t3.conn, ROOT, At::Byte(at(5)), At::Byte(at(5) + 3))
        .unwrap();
    f.tick();
    // Bob types a line at the top: every caret is carried
    f.type_at(bob, 0, "by bob\n");
    f.tick();
    // the most active connection (t3) leaves with its caret
    f.s.leave(t3.conn).unwrap();
    f.tick();

    let ros = drain(&roster);
    let late = drain(&f.s.subscribe_roster(t1.conn).unwrap());
    let cursors: Vec<(Option<u64>, bool)> = ros
        .iter()
        .filter_map(|e| match e {
            RosterEvent::Cursor {
                who: w,
                client,
                cursor,
                ..
            } if w.pid() == Some("ada") => Some((client.map(|c| st.client(c)), cursor.is_some())),
            _ => None,
        })
        .collect();
    assert_eq!(
        cursors,
        vec![
            (Some(MAX_CLIENT_ID), true),
            (Some(5), true),
            (Some(6), true),
            (Some(MAX_CLIENT_ID), true),
            (Some(5), true),
            (Some(6), true),
            (Some(6), false),
            (None, true),
        ],
        "{ros:?}"
    );
    let me = self_in(&late[0], "Ada");
    golden(
        &golden_dir().join("roster_events_devices.json"),
        &file(Value::obj([
            ("self", me.clone()),
            ("client", Value::from(st.client(t1.client))),
            (
                "events",
                Value::List(ros.iter().map(|e| roster_event(e, &mut st, &me)).collect()),
            ),
            ("final", roster_event(&late[0], &mut st, &me)),
        ])),
    );
}

// ---------- a grid doc's notes (v0.10.0) ----------

/// A small CSV-like pack for a grid doc (the grid tests' `Lite`): comma-separated fields, no
/// quoting.
struct GridLite;
const GRID_LITE: k2_mpc_base::schema::Schema = k2_mpc_base::schema::Schema {
    name: "csv-lite",
    roots: k2_mpc_base::table::ROOTS,
};

impl k2_mpc_base::session::Pack for GridLite {
    fn schema(&self) -> k2_mpc_base::schema::Schema {
        GRID_LITE
    }
    fn decode(
        &self,
        b: &[u8],
    ) -> Result<k2_mpc_base::session::PackContent, k2_mpc_base::session::Declined> {
        use k2_mpc_base::table::{Field, Shape, Table};
        let bad = |message: String| k2_mpc_base::session::Declined {
            code: "malformed",
            message,
        };
        let s = std::str::from_utf8(b).map_err(|_| bad("not UTF-8".into()))?;
        let recs: Vec<Vec<Field>> = s
            .lines()
            .map(|l| {
                if l.is_empty() {
                    Vec::new()
                } else {
                    l.split(',').map(Field::new).collect()
                }
            })
            .collect();
        let (_, edits) =
            Table::import(&recs, false, Shape::detect(&recs)).map_err(|e| bad(e.to_string()))?;
        let mut d = k2_mpc_base::doc::Doc::new(GRID_LITE).unwrap();
        d.edit_as(2, &edits).unwrap();
        Ok(k2_mpc_base::session::PackContent {
            roots: d.export_neutral().unwrap(),
            codec: vec![1],
        })
    }
    fn encode(
        &self,
        c: &[(String, k2_mpc_base::doc::Neutral)],
        _: &[u8],
    ) -> Result<Vec<u8>, k2_mpc_base::session::PackError> {
        let t = k2_mpc_base::session::View::of_content(c)
            .table()
            .map_err(|e| k2_mpc_base::session::PackError(e.to_string()))?;
        let mut out = String::new();
        for rec in t.render() {
            let f: Vec<&str> = rec.iter().map(|f| f.value.as_str()).collect();
            out.push_str(&f.join(","));
            out.push('\n');
        }
        Ok(out.into_bytes())
    }
}

/// A grid table op through the host's edit route.
fn grid_op(
    s: &Session,
    c: ConnId,
    f: impl FnOnce(
        &mut k2_mpc_base::table::Table,
    ) -> Result<Vec<k2_mpc_base::doc::Edit>, k2_mpc_base::table::TableError>,
) {
    s.edit_ops_with(c, &EditOptions::new(), |v| {
        let mut t = v.table().map_err(|e| e.to_string())?;
        f(&mut t).map_err(|e| e.to_string())
    })
    .unwrap();
}

/// A grid doc: Ada (a person who may read history) writes a cell the file never got, and an
/// outside save writes that same cell: the file wins, and Ada gets a `cell` note naming the row,
/// the column and the version cut before the save. Then Ada writes another cell of a row the
/// owner deletes and drops (`rows drop-deleted`): a `dropped_row` note, which names no version
/// (none holds a dropped row) and carries the value. Both are pushed live to Ada's own
/// subscription and wait in a later one's snapshot.
///
/// `roster_events_overwritten_grid.json`: `about`, `self`, Ada's live stream (`events`, without its
/// `CellCursor` events: the wire has no encoding of a grid's caret yet) and the
/// later subscription's snapshot (`final`). The grid's values of the same fields as a text doc's:
/// `row` set, `cells[].col` set for a cell, `restore_from` null, `not_shown` 0, `no_history`
/// false, the grid's `source` and `rule`.
#[test]
fn a_grid_docs_notes_are_golden() {
    let dir = std::env::temp_dir().join(format!("k2mb-wire-golden-{}-grid", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let path = dir.join("ws/t.csv");
    std::fs::write(&path, "a,b\nc,d\n").unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_700_000_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    host.rights
        .lock()
        .unwrap()
        .insert("ada".into(), Rights::full());
    host.rights
        .lock()
        .unwrap()
        .insert("own".into(), Rights::full().with_owner(true));
    let mut config = Config::default();
    config.edit_rate_per_s = 1_000_000;
    config.edit_burst = 1_000_000;
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, host.clone(), config);
    let key = DocKey {
        space: "ws".into(),
        path: "t.csv".into(),
    };
    let s = mgr
        .open_file(Arc::new(GridLite), &key, &path, &dir.join("ws"))
        .unwrap();
    let _guard = TmpDir(dir.clone());
    let join = |pid: &str, name: &str, colour: &str| {
        s.join(
            Participant::new(pid, name, colour, ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn
    };
    let ada = join("ada", "Ada", "#7c3aed");
    let own = join("own", "Owner", "#0f766e");
    let roster = s.subscribe_roster(ada).unwrap();
    let tick = || {
        host.now.fetch_add(300, Ordering::SeqCst);
        mgr.flush_rosters();
    };
    let k = |x: &str| k2_mpc_base::elem::Key::new(x).unwrap();
    tick();

    // 1. a cell the file never got (inside the write delay), then an outside save of that cell
    host.now.fetch_add(1_000, Ordering::SeqCst);
    grid_op(&s, ada, |t| t.set_cell(&k("r2"), &k("c2"), "LIVE"));
    host.now.fetch_add(100, Ordering::SeqCst);
    std::fs::write(&path, "a,b\nc,FILE\n").unwrap();
    s.file_changed().unwrap();
    tick();

    // 2. a cell of a row the owner deletes and drops
    host.now.fetch_add(1_000, Ordering::SeqCst);
    grid_op(&s, ada, |t| t.set_cell(&k("r1"), &k("c1"), "MINE"));
    host.now.fetch_add(1_000, Ordering::SeqCst);
    grid_op(&s, own, |t| t.delete_row(&k("r1")));
    s.drop_deleted(own).unwrap();
    tick();

    let late = drain(&s.subscribe_roster(ada).unwrap());
    let ros = drain(&roster);

    let pushed = pushes(&ros);
    assert_eq!(pushed.len(), 2, "{ros:?}");
    let (cell_note, drop_note) = (pushed[0].1, pushed[1].1);
    assert_eq!(
        (cell_note.source, cell_note.rule, cell_note.region.as_str()),
        ("an outside save", "cell", "r2/c2")
    );
    assert_eq!(cell_note.row.as_deref(), Some("r2"));
    assert_eq!((cell_note.line, cell_note.col), (Some(2), Some(2)));
    assert!(cell_note.version.is_some(), "{cell_note:?}");
    assert_eq!(
        (
            cell_note.restore_from,
            cell_note.not_shown,
            cell_note.no_history
        ),
        (None, 0, false)
    );
    assert_eq!(
        (drop_note.source, drop_note.rule),
        ("rows drop-deleted", "dropped_row")
    );
    assert!(drop_note.row.is_some(), "{drop_note:?}");
    assert!(drop_note.yours.contains("MINE"), "{drop_note:?}");
    assert_eq!(drop_note.version, None, "no version holds a dropped row");
    assert_eq!(
        (
            drop_note.restore_from,
            drop_note.not_shown,
            drop_note.no_history
        ),
        (None, 0, false)
    );
    // the same notes wait in the later snapshot
    let key_of = |o: &Overwrite| (o.change, o.row.clone());
    assert_eq!(
        waiting(&late[0]).iter().map(key_of).collect::<Vec<_>>(),
        pushed.iter().map(|(_, o)| key_of(o)).collect::<Vec<_>>()
    );

    let mut st = Stable::default();
    let me = self_in(&late[0], "Ada");
    golden(
        &golden_dir().join("roster_events_overwritten_grid.json"),
        &file(Value::obj([
            (
                "about",
                Value::from(
                    "A grid doc's notes: an outside save wrote a cell Ada had written that the file \
                     never got (rule `cell`, with its row, column and the version cut before the \
                     save), then the owner dropped a deleted row holding Ada's cell (`rows \
                     drop-deleted`, rule `dropped_row`, which no version holds). `events` is Ada's \
                     live stream, without its cell-cursor events (the wire doesn't encode a \
                     grid's caret yet); `final` is a later subscription's snapshot with both notes \
                     waiting.",
                ),
            ),
            ("self", me.clone()),
            (
                "events",
                Value::List(
                    ros.iter()
                        .filter(|e| !matches!(e, RosterEvent::CellCursor { .. }))
                        .map(|e| roster_event(e, &mut st, &me))
                        .collect(),
                ),
            ),
            ("final", roster_event(&late[0], &mut st, &me)),
        ])),
    );
    drop(s);
    let _ = mgr.release();
}

// ---------- element presence (selection, pointer, viewport) ----------

/// A keyed doc (not a table) with `items` i1..i6, and the session's fixture.
fn keyed_fx(tag: &str) -> (TmpDir, Arc<TestHost>, SessionManager, Arc<Session>) {
    let dir = std::env::temp_dir().join(format!("k2mb-wire-golden-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_700_000_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    let mut config = Config::default();
    config.edit_rate_per_s = 1_000_000;
    config.edit_burst = 1_000_000;
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, host.clone(), config);
    let s = mgr
        .open(
            Arc::new(keyed_pack::Keyed),
            &DocKey {
                space: "ws".into(),
                path: "doc.keyed".into(),
            },
            Some(b"i1\ni2\ni3\ni4\ni5\ni6\n"),
        )
        .unwrap();
    (TmpDir(dir), host, mgr, s)
}

fn key_of(s: &str) -> Key {
    Key::new(s).unwrap()
}

fn items(keys: &[&str]) -> Option<ElemSelection> {
    Some(ElemSelection::new(
        keyed_pack::ITEMS,
        keys.iter().map(|k| key_of(k)).collect(),
    ))
}

fn delete_item(s: &Session, c: ConnId, key: &str) {
    s.edit_ops(
        c,
        vec![Edit::List {
            root: keyed_pack::ITEMS.into(),
            ops: vec![ListOp::Delete { key: key_of(key) }],
        }],
        &EditOptions::new(),
    )
    .unwrap();
}

/// One person on two connections and another person: each sets a selection, a pointer and a
/// viewport; an element is deleted, so it leaves the selection; a connection leaves, so its
/// values go (`null`), followed by the participant's own value with no `client`.
///
/// `roster_events_elem_presence.json`: `self`, `client`, the stream, and `final`, a later
/// subscription's snapshot.
#[test]
fn element_presence_is_golden() {
    let (_dir, host, mgr, s) = keyed_fx("elem");
    let tick = || {
        host.now.fetch_add(300, Ordering::SeqCst);
        mgr.flush_rosters();
    };
    let person = |pid: &str, name: &str, colour: &str| {
        s.join(
            Participant::new(pid, name, colour, ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
    };
    let a1 = person("ada", "Ada", "#7c3aed");
    let a2 = person("ada", "Ada", "#7c3aed");
    let b = person("bob", "Bob", "#16a34a");
    let mut st = Stable::default();
    st.clients.insert(a1.client, 4);
    st.clients.insert(a2.client, 5);
    st.clients.insert(b.client, 6);
    let roster = s.subscribe_roster(a1.conn).unwrap();
    tick();

    s.set_selection(a1.conn, items(&["i1", "i2"])).unwrap();
    s.set_pointer(a1.conn, Some(Point::new(120.0, -40.0).unwrap()))
        .unwrap();
    s.set_viewport(a1.conn, Some(Rect::new(0.0, 0.0, 800.0, 600.0).unwrap()))
        .unwrap();
    tick();
    host.now.fetch_add(10, Ordering::SeqCst);
    s.set_selection(a2.conn, items(&["i3"])).unwrap();
    s.set_pointer(a2.conn, Some(Point::new(5.0, 6.0).unwrap()))
        .unwrap();
    s.set_selection(b.conn, items(&["i2", "i4"])).unwrap();
    tick();
    // an element leaves every selection that held it
    delete_item(&s, b.conn, "i2");
    tick();
    // a connection leaves with its values
    s.leave(a2.conn).unwrap();
    tick();
    // cleared
    s.set_viewport(a1.conn, None).unwrap();
    s.set_pointer(a1.conn, None).unwrap();
    tick();

    let ros = drain(&roster);
    let late = drain(&s.subscribe_roster(a1.conn).unwrap());
    for k in ["selection", "pointer", "viewport"] {
        assert!(
            ros.iter().any(|e| matches!(
                (k, e),
                ("selection", RosterEvent::Selection { .. })
                    | ("pointer", RosterEvent::Pointer { .. })
                    | ("viewport", RosterEvent::Viewport { .. })
            )),
            "no {k} event in {ros:?}"
        );
    }
    let me = self_in(&late[0], "Ada");
    golden(
        &golden_dir().join("roster_events_elem_presence.json"),
        &file(Value::obj([
            ("self", me.clone()),
            ("client", Value::from(st.client(a1.client))),
            (
                "events",
                Value::List(ros.iter().map(|e| roster_event(e, &mut st, &me)).collect()),
            ),
            ("final", roster_event(&late[0], &mut st, &me)),
        ])),
    );
}

/// xorshift64*, as `client_vectors.rs`: the trace doesn't depend on a crate's generator.
fn next(state: &mut u64) -> u64 {
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    state.wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// Seeded traces for the JS client: random selections, pointers, viewports, deletes, joins and
/// leaves, flushed at random. `roster_trace_elem_presence.json`: per seed, the stream a
/// subscriber got and `final`, a later subscription's snapshot. `client/test/presence-channel.test.ts`
/// replays each over a network with latency: a `PresenceChannel` folds the stream to `final`.
#[test]
fn element_presence_traces_are_golden() {
    let mut traces = Vec::new();
    for seed in 1..=8u64 {
        let (_dir, host, mgr, s) = keyed_fx(&format!("trace{seed}"));
        let mut r = seed.wrapping_mul(0x2545_F491_4F6C_DD1D) | 1;
        let mut st = Stable::default();
        let mut next_client = 10u64;
        let mut join = |pid: &str, st: &mut Stable| {
            let j = s
                .join(
                    Participant::new(pid, pid, "#123456", ParticipantKind::Person, "tab"),
                    Stay::Held,
                )
                .unwrap();
            st.clients.insert(j.client, next_client);
            next_client += 1;
            j
        };
        let watcher = join("watcher", &mut st);
        let roster = s.subscribe_roster(watcher.conn).unwrap();
        let mut conns = vec![
            join("ada", &mut st).conn,
            join("ada", &mut st).conn,
            join("bob", &mut st).conn,
        ];
        let mut live: Vec<&str> = vec!["i1", "i2", "i3", "i4", "i5", "i6"];
        for _ in 0..60 {
            host.now.fetch_add(next(&mut r) % 50, Ordering::SeqCst);
            let c = conns[(next(&mut r) % conns.len() as u64) as usize];
            match next(&mut r) % 12 {
                0..=2 if !live.is_empty() => {
                    let a = live[(next(&mut r) % live.len() as u64) as usize];
                    let b = live[(next(&mut r) % live.len() as u64) as usize];
                    s.set_selection(c, items(&[a, b])).unwrap();
                }
                3 => {
                    s.set_selection(c, None).unwrap();
                }
                4..=6 => {
                    let (x, y) = (next(&mut r) % 2000, next(&mut r) % 2000);
                    let p = Point::new(x as f64 - 1000.0, y as f64).unwrap();
                    s.set_pointer(c, Some(p)).unwrap();
                }
                7 => {
                    s.set_pointer(c, None).unwrap();
                }
                8 => {
                    let (x, w) = (next(&mut r) % 500, next(&mut r) % 500);
                    let v = Rect::new(x as f64, 0.0, w as f64, 300.0).unwrap();
                    s.set_viewport(c, Some(v)).unwrap();
                }
                9 if live.len() > 1 => {
                    let k = live.remove((next(&mut r) % live.len() as u64) as usize);
                    delete_item(&s, conns[conns.len() - 1], k);
                }
                10 if conns.len() > 1 => {
                    s.leave(conns.remove(0)).unwrap();
                }
                _ => {
                    host.now.fetch_add(300, Ordering::SeqCst);
                    mgr.flush_rosters();
                }
            }
        }
        host.now.fetch_add(300, Ordering::SeqCst);
        mgr.flush_rosters();
        let ros = drain(&roster);
        let late = drain(&s.subscribe_roster(watcher.conn).unwrap());
        let me = self_in(&late[0], "watcher");
        traces.push(Value::obj([
            ("seed", Value::from(seed)),
            ("self", me.clone()),
            (
                "events",
                Value::List(ros.iter().map(|e| roster_event(e, &mut st, &me)).collect()),
            ),
            ("final", roster_event(&late[0], &mut st, &me)),
        ]));
    }
    golden(
        &golden_dir().join("roster_trace_elem_presence.json"),
        &file(Value::obj([("traces", Value::List(traces))])),
    );
}

// ---------- claims on elements ----------

/// A keyed doc that isn't a table: a list `items` and a Map `props` declared composite on it.
struct ItemsLite;
const ITEMS_LITE: k2_mpc_base::schema::Schema = k2_mpc_base::schema::Schema {
    name: "items-lite",
    roots: &[
        ("items", k2_mpc_base::schema::Kind::MovableList),
        ("props", k2_mpc_base::schema::Kind::Map),
    ],
};

impl k2_mpc_base::session::Pack for ItemsLite {
    fn schema(&self) -> k2_mpc_base::schema::Schema {
        ITEMS_LITE
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        &[("props", "items")]
    }
    fn decode(
        &self,
        b: &[u8],
    ) -> Result<k2_mpc_base::session::PackContent, k2_mpc_base::session::Declined> {
        use k2_mpc_base::doc::Neutral;
        let items = std::str::from_utf8(b)
            .unwrap()
            .lines()
            .map(|l| (k2_mpc_base::Key::new(l).unwrap(), k2_mpc_base::Value::Null))
            .collect();
        Ok(k2_mpc_base::session::PackContent {
            roots: vec![
                ("items".into(), Neutral::List(items)),
                ("props".into(), Neutral::Map(Default::default())),
            ],
            codec: Vec::new(),
        })
    }
    fn encode(
        &self,
        _: &[(String, k2_mpc_base::doc::Neutral)],
        _: &[u8],
    ) -> Result<Vec<u8>, k2_mpc_base::session::PackError> {
        Ok(Vec::new())
    }
}

/// Ada (a person) subscribes. An agent claims two items with a note, then one more with `hold`.
/// Ada deletes the held item: that claim ends `deleted`. The agent releases the first claim and
/// takes another, still live: a later subscription's snapshot names it (`elem_claims`), so a late
/// subscriber sees who holds what.
///
/// `roster_events_claims_elems.json`: `about`, `self`, the stream, and `final`, that later
/// snapshot. Only new kinds and fields: no other golden changes.
#[test]
fn element_claims_are_golden() {
    let dir = std::env::temp_dir().join(format!("k2mb-wire-elems-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _dir = TmpDir(dir.clone());
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_700_000_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, host.clone(), Config::default());
    let s = mgr
        .open(
            Arc::new(ItemsLite),
            &DocKey {
                space: "ws".into(),
                path: "a.items".into(),
            },
            Some(b"i1\ni2\ni3\ni4\n"),
        )
        .unwrap();
    let tick = || {
        host.now.fetch_add(300, Ordering::SeqCst);
        mgr.flush_rosters();
    };
    let keys = |v: &[&str]| -> Vec<k2_mpc_base::Key> {
        v.iter()
            .map(|k| k2_mpc_base::Key::new(*k).unwrap())
            .collect()
    };
    let ada = s
        .join(
            Participant::new("ada", "Ada", "#7c3aed", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap();
    let bot = s
        .join(
            Participant::new("bot", "Bot", "#0891b2", ParticipantKind::Agent, "cli"),
            Stay::Held,
        )
        .unwrap();
    let mut st = Stable::default();
    st.clients.insert(ada.client, MAX_CLIENT_ID);
    st.clients.insert(bot.client, 5);
    let roster = s.subscribe_roster(ada.conn).unwrap();
    let first = s
        .claim_elems(
            bot.conn,
            "items",
            &keys(&["i2", "i1"]),
            None,
            Some("moving".into()),
            false,
        )
        .unwrap();
    tick();
    s.claim_elems(bot.conn, "items", &keys(&["i3"]), Some(5_000), None, true)
        .unwrap();
    tick();
    s.edit_ops(
        ada.conn,
        vec![k2_mpc_base::doc::Edit::List {
            root: "items".into(),
            ops: vec![k2_mpc_base::elem::ListOp::Delete {
                key: keys(&["i3"]).remove(0),
            }],
        }],
        &EditOptions::new(),
    )
    .unwrap();
    tick();
    s.release_claim(bot.conn, Some(first.id)).unwrap();
    tick();
    s.claim_elems(bot.conn, "items", &keys(&["i4"]), None, None, true)
        .unwrap();
    tick();
    let ros = drain(&roster);
    let late = drain(&s.subscribe_roster(ada.conn).unwrap());
    let me = who(&Who::Pid("ada".into()));
    golden(
        &golden_dir().join("roster_events_claims_elems.json"),
        &file(Value::obj([
            (
                "about",
                Value::from(
                    "Claims on elements: `claim_started_elems`, `claim_ended` with `how: deleted` \
                     and `released`, and `final`, a later subscription's snapshot, whose entry \
                     `elem_claims` names the claim still live.",
                ),
            ),
            ("self", me.clone()),
            (
                "events",
                Value::List(ros.iter().map(|e| roster_event(e, &mut st, &me)).collect()),
            ),
            ("final", roster_event(&late[0], &mut st, &me)),
        ])),
    );
    // the scenario did what the file says
    let kinds: Vec<&str> = ros
        .iter()
        .filter_map(|e| match e {
            RosterEvent::ClaimStartedElems { .. } => Some("started"),
            RosterEvent::ClaimEnded { how, .. } => Some(how.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        kinds,
        ["started", "started", "deleted", "released", "started"]
    );
}
