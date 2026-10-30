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

use super::{change, report, roster_event};

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
    same("roster_events.json", ROSTER_EVENTS, &ada_file);
    same("roster_events_app.json", ROSTER_EVENTS_APP, &app_file);

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

/// Pretty JSON without its `"next": …` lines (each is one line: JSON strings
/// hold no raw line break).
fn without_next(pretty: &str) -> String {
    pretty.lines().filter(|l| !l.trim_start().starts_with("\"next\": ")).map(|l| format!("{l}\n")).collect()
}
