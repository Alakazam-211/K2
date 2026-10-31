//! K2's Live encoder against the kit's golden JSON (`client/test/golden/*.json`
//! at kit main `bea1885`, copied to `tests/golden/live_wire/`), byte for byte.
//!
//! The kit wrote those files from one scenario (`crates/base/tests/wire_golden.rs`):
//! a person, an agent on her behalf, a second person and an App viewer; live
//! edits at a base, an undo, carets, away, a claim, a rights change and a leave.
//! This replays the same scenario on the vendored kit and encodes what it yields
//! with K2's encoder ([`super`]), then applies the kit's own two rewrites (random
//! client ids → fixed numbers; claim ids numbered in order of appearance) and
//! compares the pretty JSON with each file.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use k2_mpc_base::errors::{ErrorReport, Value};
use k2_mpc_base::session::{
    At, AuditEvent, ChangeEvent, Config, ConnId, EditOptions, Host, LineRange, Participant, ParticipantKind, Rights,
    RosterEvent, Session, SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore, Seq};
use k2_mpc_base::text::Hunk;
use k2_mpc_base::MAX_CLIENT_ID;

use super::{change, report, roster_event, roster_event_for};

const ROOT: &str = TextFilePack::ROOT;
const DOC: &str = "# Notes\n\none\ntwo\nthree\n";
const TRICKY: &str = "say \"hi\"\t\\ caf\u{e9} \u{1F600}\u{1}\n";

const CHANGE_EVENTS: &str = include_str!("../../tests/golden/live_wire/change_events.json");
const CHANGE_EVENTS_APP: &str = include_str!("../../tests/golden/live_wire/change_events_app.json");
const ROSTER_EVENTS: &str = include_str!("../../tests/golden/live_wire/roster_events.json");
const ROSTER_EVENTS_APP: &str = include_str!("../../tests/golden/live_wire/roster_events_app.json");
const ERRORS: &str = include_str!("../../tests/golden/live_wire/errors.json");

struct TestHost {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        self.rights.lock().expect("rights").get(&who.pid).copied().unwrap_or(Rights::editor())
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

/// The kit's rewrites: fixed client ids, claim ids in order of appearance.
#[derive(Default)]
struct Stable {
    clients: HashMap<u64, u64>,
    claims: HashMap<String, u64>,
}

impl Stable {
    fn rewrite(&mut self, v: &mut Value) {
        match v {
            Value::Obj(pairs) => {
                for (k, val) in pairs.iter_mut() {
                    match (k.as_str(), &*val) {
                        ("client", Value::UInt(c)) => {
                            let fixed = *self.clients.get(c).unwrap_or_else(|| panic!("client {c} isn't one of the scenario's"));
                            *val = Value::from(fixed);
                        }
                        ("client", Value::Int(c)) => {
                            let c = *c as u64;
                            let fixed = *self.clients.get(&c).unwrap_or_else(|| panic!("client {c} isn't one of the scenario's"));
                            *val = Value::from(fixed);
                        }
                        ("claim", Value::Str(s)) if s.starts_with("claim-") => {
                            let n = self.claims.len() as u64 + 1;
                            let k = *self.claims.entry(s.clone()).or_insert(n);
                            *val = Value::from(format!("claim-{k}"));
                        }
                        _ => self.rewrite(val),
                    }
                }
            }
            Value::List(items) => items.iter_mut().for_each(|i| self.rewrite(i)),
            _ => {}
        }
    }
}

fn drain<T>(rx: &Receiver<T>) -> Vec<T> {
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

fn file(v: Value) -> String {
    let mut s = v.to_json_pretty();
    s.push('\n');
    s
}

fn same(name: &str, want: &str, got: &str) {
    assert!(want == got, "{name}: K2's encoding differs from the kit's golden file\n--- K2:\n{got}\n--- golden:\n{want}");
}

fn h(start: usize, end: usize, insert: &str) -> Hunk {
    Hunk { start, end, insert: insert.into() }
}

struct Fx {
    dir: PathBuf,
    host: Arc<TestHost>,
    mgr: SessionManager,
    s: Arc<Session>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Fx {
    fn new() -> Fx {
        let dir = std::env::temp_dir().join(format!("k2-live-wire-golden-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        let host = Arc::new(TestHost { now: AtomicU64::new(1_700_000_000_000), rights: Mutex::new(HashMap::new()) });
        let mut config = Config::default();
        config.live_window_events = 2;
        config.edit_rate_per_s = 1_000_000;
        config.edit_burst = 1_000_000;
        let store = Arc::new(DocStore::open(dir.join("docs.redb")).expect("store"));
        let mgr = SessionManager::new(store, host.clone(), config);
        let s = mgr
            .open(
                Arc::new(TextFilePack::new("markdown", 4 << 20)),
                &DocKey { space: "ws".into(), path: "notes.md".into() },
                Some(DOC.as_bytes()),
            )
            .expect("open");
        Fx { dir, host, mgr, s }
    }
    fn tick(&self) {
        self.host.now.fetch_add(300, Ordering::SeqCst);
        self.mgr.flush_rosters();
    }
    fn version(&self) -> Seq {
        self.s.status().expect("status").version
    }
    fn text(&self, c: ConnId) -> String {
        self.s.read(c).expect("read").text_by_root.into_iter().find(|(r, _)| r == ROOT).expect("root").1
    }
    fn edit(&self, c: ConnId, base: Seq, hunks: Vec<Hunk>) -> Result<Vec<Hunk>, ErrorReport> {
        self.host.now.fetch_add(7, Ordering::SeqCst);
        self.s.edit_at_base(c, ROOT, base, hunks, &EditOptions::new()).map(|r| r.hunks)
    }
}

fn changes_file(evs: &[ChangeEvent], v0: Seq, text0: &str, v_end: Seq, final_text: &str, st: &mut Stable) -> String {
    let mut events = Value::List(evs.iter().map(change).collect());
    st.rewrite(&mut events);
    file(Value::obj([
        ("root", Value::from(ROOT)),
        ("version", Value::from(v0)),
        ("text", Value::from(text0)),
        ("events", events),
        ("final_version", Value::from(v_end)),
        ("final_text", Value::from(final_text)),
    ]))
}

#[test]
fn k2s_live_encoder_writes_the_kits_golden_json() {
    let f = Fx::new();
    let join = |p: Participant| f.s.join(p, Stay::Held).expect("join");
    let ada = join(Participant::new("ada", "Ada", "#7c3aed", ParticipantKind::Person, "tab"));
    let agent = join(Participant::new("agent-7", "Helper", "#0ea5e9", ParticipantKind::Agent, "cli").on_behalf_of("ada"));
    let app = join(Participant::new("widget", "Widget", "#64748b", ParticipantKind::App, "app"));
    let mut st = Stable::default();
    st.clients.insert(ada.client, MAX_CLIENT_ID);
    st.clients.insert(agent.client, 2);
    st.clients.insert(app.client, 3);
    let (ada, agent, app) = (ada.conn, agent.conn, app.conn);

    let changes = f.s.subscribe(ada).expect("subscribe");
    let changes_app = f.s.subscribe(app).expect("subscribe app");
    let roster = f.s.subscribe_roster(ada).expect("roster");
    let roster_app = f.s.subscribe_roster(app).expect("roster app");
    let v0 = f.version();
    let text0 = f.text(ada);
    assert_eq!(text0, DOC);

    let bob = join(Participant::new("bob", "Bob", "#16a34a", ParticipantKind::Person, "tab"));
    st.clients.insert(bob.client, 4);
    let bob = bob.conn;
    f.tick();

    let mut errors: Vec<(&str, ErrorReport)> = Vec::new();
    f.edit(ada, v0, vec![h(9, 9, "zero\n")]).expect("ada's line");
    let applied = f.edit(agent, v0, vec![h(13, 17, ""), h(23, 23, TRICKY)]).expect("the agent's edit");
    assert_eq!(applied, vec![h(18, 22, ""), h(28, 28, TRICKY)]);
    let e = f.edit(ada, v0, vec![h(13, 16, "TWO")]).expect_err("a write conflict");
    assert_eq!(e.id(), "MPC-E111");
    errors.push(("write_conflict", e));
    let v = f.version();
    let e = f.edit(ada, v, vec![h(10_000, 10_000, "x")]).expect_err("a hunk past the end");
    assert_eq!(e.id(), "MPC-E151");
    errors.push(("bad_hunk", e));
    let undone = f.s.undo(ada, 1).expect("undo");
    assert!(format!("{undone:?}").starts_with("Applied"), "{undone:?}");
    let v_end = f.version();
    let e = f.s.events_since(ada, v0).expect_err("past the window");
    assert_eq!(e.id(), "MPC-E112");
    errors.push(("base_expired", e));
    assert_eq!(f.s.events_since(ada, v_end - 2).expect("since").len(), 2);

    let text = f.text(ada);
    let emoji = text.find('\u{1F600}').expect("emoji") + 4;
    f.s.set_cursor(ada, ROOT, At::Byte(0), At::Byte(7)).expect("ada's selection");
    f.s.set_cursor(bob, ROOT, At::Byte(emoji), At::Byte(emoji)).expect("bob's caret");
    let e = f.s.set_cursor(bob, ROOT, At::Byte(emoji - 1), At::Byte(emoji - 1)).expect_err("off a boundary").report();
    assert_eq!(e.id(), "MPC-E114");
    errors.push(("bad_position", e));
    f.tick();

    f.s.set_visible(bob, false).expect("hidden");
    f.host.now.fetch_add(60_001, Ordering::SeqCst);
    f.tick();

    let claim = f
        .s
        .claim(agent, LineRange { root: ROOT.into(), first: 1, last: 2 }, Some(30_000), Some("tidying the heading".into()), false)
        .expect("claim");
    f.tick();
    f.s.release_claim(agent, Some(claim.id)).expect("release");
    f.tick();

    f.host.rights.lock().expect("rights").insert("bob".into(), Rights::read_only());
    f.s.recheck_rights().expect("recheck");
    f.tick();
    f.s.leave(bob).expect("leave");
    f.tick();

    // The reports a host builds itself, through K2's builders.
    errors.push(("bad_edit_frame_cap", super::over_cap(4_194_400)));
    errors.push(("declined_too_large", super::too_large(6_291_502).error.expect("E401")));
    errors.push(("history_lost", report("MPC-E206", "nothing to undo: your undo history was lost when the doc reloaded")));

    let final_text = f.text(ada);
    let evs = drain(&changes);
    assert_eq!(evs.len(), 3, "{evs:?}");
    same("change_events.json", CHANGE_EVENTS, &changes_file(&evs, v0, &text0, v_end, &final_text, &mut st));
    let evs_app = drain(&changes_app);
    assert_eq!(evs_app.len(), 3);
    same("change_events_app.json", CHANGE_EVENTS_APP, &changes_file(&evs_app, v0, &text0, v_end, &final_text, &mut st));

    let ros = drain(&roster);
    let ros_app = drain(&roster_app);
    let end = drain(&f.s.subscribe_roster(ada).expect("final roster"));
    let end_app = drain(&f.s.subscribe_roster(app).expect("final roster app"));
    let lens = |me: &str, evs: &[RosterEvent], end: &[RosterEvent], st: &mut Stable| -> String {
        let self_who = match &end[0] {
            RosterEvent::Snapshot { members, .. } => members
                .iter()
                .find(|m| m.entry.participant.name == me)
                .map_or(Value::Null, |m| Value::from(m.who.to_string())),
            other => panic!("{other:?}"),
        };
        let mut events = Value::List(evs.iter().map(|e| roster_event(e).expect("a known roster event")).collect());
        st.rewrite(&mut events);
        let mut fin = roster_event(&end[0]).expect("snapshot");
        st.rewrite(&mut fin);
        file(Value::obj([("self", self_who), ("events", events), ("final", fin)]))
    };
    let ada_file = lens("Ada", &ros, &end, &mut st);
    let app_file = lens("Widget", &ros_app, &end_app, &mut st);
    // A roster's members are a set (K2 Cores, ahead of v0.9.8.2): compared
    // by handle, never by position. Every other byte as is.
    same("roster_events.json", &members_by_who(ROSTER_EVENTS), &members_by_who(&ada_file));
    same("roster_events_app.json", &members_by_who(ROSTER_EVENTS_APP), &members_by_who(&app_file));

    let got = file(Value::obj(errors.iter().map(|(n, r)| (n.to_string(), r.to_value())).collect::<Vec<_>>()));
    // Errors are keyed on their id, never on their `next`: that is the
    // registry's guidance text, which the kit rewrites between releases (kit
    // main's MPC-E111 `next` is newer than the vendored v0.9.7.1's). Every
    // other byte is compared: the ids, exits, messages, rules, details and
    // field order.
    let ids = |text: &str| -> Vec<String> {
        text.lines().filter_map(|l| l.trim_start().strip_prefix("\"id\": ")).map(str::to_string).collect()
    };
    assert_eq!(ids(&got), ids(ERRORS), "errors.json: the same error ids, in order");
    same("errors.json (next aside)", &without_next(ERRORS), &without_next(&got));
}

/// A JSON value with its object keys in file order (serde_json's own map
/// sorts them), as the kit's [`Value`], so a re-print is the kit's bytes.
struct InOrder(Value);

impl<'de> serde::Deserialize<'de> for InOrder {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = InOrder;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON without fractions")
            }
            fn visit_unit<E>(self) -> Result<InOrder, E> {
                Ok(InOrder(Value::Null))
            }
            fn visit_bool<E>(self, b: bool) -> Result<InOrder, E> {
                Ok(InOrder(Value::Bool(b)))
            }
            fn visit_i64<E>(self, n: i64) -> Result<InOrder, E> {
                Ok(InOrder(Value::Int(n)))
            }
            fn visit_u64<E>(self, n: u64) -> Result<InOrder, E> {
                Ok(InOrder(Value::UInt(n)))
            }
            fn visit_str<E>(self, s: &str) -> Result<InOrder, E> {
                Ok(InOrder(Value::Str(s.to_owned())))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut a: A) -> Result<InOrder, A::Error> {
                let mut v = Vec::new();
                while let Some(InOrder(x)) = a.next_element()? {
                    v.push(x);
                }
                Ok(InOrder(Value::List(v)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut a: A) -> Result<InOrder, A::Error> {
                let mut v = Vec::new();
                while let Some((k, InOrder(x))) = a.next_entry::<String, InOrder>()? {
                    v.push((k, x));
                }
                Ok(InOrder(Value::Obj(v)))
            }
        }
        d.deserialize_any(V)
    }
}

/// Pretty JSON with every roster's `members` sorted by `who`.
fn members_by_who(pretty: &str) -> String {
    fn sort(v: &mut Value) {
        match v {
            Value::Obj(pairs) => {
                for (k, val) in pairs.iter_mut() {
                    if let (true, Value::List(items)) = (k == "members", &mut *val) {
                        items.sort_by(|a, b| {
                            let who = |m: &Value| m.get("who").and_then(Value::as_str).expect("a member's who").to_owned();
                            who(a).cmp(&who(b))
                        });
                    }
                    sort(val);
                }
            }
            Value::List(items) => items.iter_mut().for_each(sort),
            _ => {}
        }
    }
    let InOrder(mut v) = serde_json::from_str(pretty).expect("roster file JSON");
    sort(&mut v);
    file(v)
}

/// A pretty file whose `members` came in another order is the same roster.
#[test]
fn rosters_compare_by_handle_not_position() {
    let a = "{\n  \"members\": [\n    {\n      \"who\": \"b\",\n      \"n\": 2\n    },\n    {\n      \"who\": \"a\",\n      \"n\": 1\n    }\n  ]\n}\n";
    let b = "{\n  \"members\": [\n    {\n      \"who\": \"a\",\n      \"n\": 1\n    },\n    {\n      \"who\": \"b\",\n      \"n\": 2\n    }\n  ]\n}\n";
    assert_eq!(members_by_who(a), members_by_who(b));
    assert_eq!(members_by_who(b), b, "a sorted file prints back byte for byte");
    let c = b.replace("\"n\": 2", "\"n\": 3");
    assert_ne!(members_by_who(a), members_by_who(&c), "a member's fields still count");
}

// ── `overwritten` (kit v0.9.7.2): an outside save may have removed text ──

const OW_V0: &str = "intro\nmiddle\nend\n";
const OW_LINE: &str = "ada's line\n";

/// A file-backed doc: Ada types a line through the kit, it reaches the file,
/// then an editor holding the old text saves it plus a line of its own.
/// Returns Ada's roster events after the save, and the session for a late
/// subscription.
fn stale_save_over_ada() -> (PathBuf, SessionManager, Arc<Session>, ConnId, Vec<RosterEvent>) {
    let dir = std::env::temp_dir().join(format!("k2-live-wire-ow-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dir.join("ws")).expect("dir");
    let file_path = dir.join("ws/notes.md");
    std::fs::write(&file_path, OW_V0).expect("file");
    let host = Arc::new(TestHost { now: AtomicU64::new(1_700_000_000_000), rights: Mutex::new(HashMap::new()) });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).expect("store"));
    let mgr = SessionManager::new(store, host.clone(), Config::default());
    let key = DocKey { space: "ws".into(), path: "notes.md".into() };
    let s = mgr.open_file(Arc::new(TextFilePack::new("markdown", 4 << 20)), &key, &file_path, &dir.join("ws")).expect("open");
    let ada = s.join(Participant::new("ada", "Ada", "#7c3aed", ParticipantKind::Person, "tab"), Stay::Held).expect("join").conn;
    let roster = s.subscribe_roster(ada).expect("roster");
    let argv: Vec<String> = vec!["insert".into(), "--line".into(), "2".into()];
    s.run_tool(ada, ROOT, &argv, OW_LINE.as_bytes()).expect("ada's line");
    s.flush(ada).expect("written back");
    host.now.fetch_add(10_000, Ordering::SeqCst);
    std::fs::write(&file_path, format!("{OW_V0}edited in the editor\n")).expect("stale save");
    s.file_changed().expect("taken in");
    host.now.fetch_add(1_000, Ordering::SeqCst);
    s.flush_roster().expect("roster flush");
    let evs = drain(&roster);
    (dir, mgr, s, ada, evs)
}

/// The live `overwritten` event: the kit's `Overwrite` field for field, its
/// first place and `also` as `cells`, and `message` its Display text.
#[test]
fn overwritten_events_carry_the_kits_overwrite() {
    let (dir, _mgr, s, ada, evs) = stale_save_over_ada();
    let (to, o) = evs
        .iter()
        .find_map(|e| match e {
            RosterEvent::Overwritten { to, overwrite, .. } => Some((to.to_string(), overwrite.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("Ada was told: {evs:?}"));
    assert_eq!(o.yours, OW_LINE, "the scenario removed Ada's own line");
    assert!(!o.is_rollup());
    let ev = evs.iter().find(|e| matches!(e, RosterEvent::Overwritten { .. })).expect("the event");
    let got = roster_event(ev).expect("encoded").to_json();
    let opt = |v: Option<u64>| v.map_or(Value::Null, Value::from);
    let mut cells = vec![Value::obj([
        ("region", Value::from(o.region.as_str())),
        ("col", o.col.map_or(Value::Null, Value::from)),
        ("yours", Value::from(o.yours.as_str())),
        ("file", Value::from(o.file.as_str())),
        ("cut", Value::from(o.cut)),
    ])];
    for c in &o.also {
        cells.push(Value::obj([
            ("region", Value::from(c.region.as_str())),
            ("col", c.col.map_or(Value::Null, Value::from)),
            ("yours", Value::from(c.yours.as_str())),
            ("file", Value::from(c.file.as_str())),
            ("cut", Value::from(c.cut)),
        ]));
    }
    let want = Value::obj([
        ("kind", Value::from("overwritten")),
        ("to", Value::from(to.as_str())),
        ("change", Value::from(o.change)),
        ("version", opt(o.version)),
        ("restore_from", opt(o.restore_from)),
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
    .to_json();
    assert_eq!(got, want);
    let message = o.to_string();
    assert!(message.starts_with("An outside save may have removed your text"), "{message}");
    assert!(message.contains("ada's line"), "the message holds Ada's text: {message}");
    drop((s, ada));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A snapshot's waiting notes: the kit's bare `Overwrite` list, each encoded
/// as the live event is (with `kind` and `to`, the subscriber), so a client
/// handles both the same way; a snapshot without notes has no
/// `overwritten` key (the kit's golden bytes).
#[test]
fn snapshot_entries_take_the_live_events_shape() {
    let (dir, _mgr, s, ada, evs) = stale_save_over_ada();
    let live = evs
        .iter()
        .find(|e| matches!(e, RosterEvent::Overwritten { .. }))
        .map(|e| roster_event(e).expect("encoded").to_json())
        .unwrap_or_else(|| panic!("Ada was told: {evs:?}"));
    // A push doesn't consume the note: a late subscription's snapshot has it.
    let late = drain(&s.subscribe_roster(ada).expect("late roster"));
    let snap = roster_event_for(&late[0], Value::from("ada")).expect("snapshot");
    assert_eq!(snap.get("kind").and_then(Value::as_str), Some("snapshot"));
    match snap.get("overwritten") {
        Some(Value::List(entries)) => {
            assert_eq!(entries.len(), 1, "{}", snap.to_json());
            assert_eq!(entries[0].to_json(), live, "a snapshot entry is the live event");
        }
        other => panic!("no overwritten list: {other:?} in {}", snap.to_json()),
    }
    // Someone else's snapshot holds none, and no key at all.
    let bob = s.join(Participant::new("bob", "Bob", "#16a34a", ParticipantKind::Person, "tab"), Stay::Held).expect("bob").conn;
    let bobs = drain(&s.subscribe_roster(bob).expect("bob's roster"));
    let snap = roster_event_for(&bobs[0], Value::from("bob")).expect("snapshot");
    assert!(snap.get("overwritten").is_none(), "{}", snap.to_json());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Pretty JSON without its `"next": …` lines (each is one line: JSON strings
/// hold no raw line break).
fn without_next(pretty: &str) -> String {
    pretty.lines().filter(|l| !l.trim_start().starts_with("\"next\": ")).map(|l| format!("{l}\n")).collect()
}
