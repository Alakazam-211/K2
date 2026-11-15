//! The keyed watch on the docs socket, against the vendored kit: which docs
//! list it, the frames a `watch` gets (the kit's, byte for byte), one
//! numbered `ops_change` per change, and the refusals.

use std::collections::BTreeMap;
use std::sync::Arc;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{Anchor, ListOp};
use k2_mpc_base::session::{
    AuditEvent, ConnId, Declined, Host, Pack, PackContent, PackError, Participant, ParticipantKind, Rights, Session,
    SessionManager, Stay, TextFilePack, WatchOptions, KEYED_WATCH_FEATURE,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::{Edit, Key, Kind, Schema, Value as Elem};
use serde_json::Value as J;

use super::*;

/// A small board: `shapes` (a MovableList of each shape's kind) and
/// `fields` (a Map). Its file is one `key=kind` line a shape; the fields
/// aren't kept in the file (nothing here reads it back).
struct Board;

const SHAPES: &str = "shapes";

impl Pack for Board {
    fn schema(&self) -> Schema {
        Schema { name: "board-k2", roots: &[(SHAPES, Kind::MovableList), ("fields", Kind::Map)] }
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let s = std::str::from_utf8(b).map_err(|e| Declined { code: "malformed", message: e.to_string() })?;
        let mut shapes = Vec::new();
        for line in s.lines().filter(|l| !l.is_empty()) {
            let (k, v) = line.split_once('=').ok_or(Declined { code: "malformed", message: format!("bad line {line:?}") })?;
            shapes.push((Key::new(k).map_err(|e| Declined { code: "malformed", message: e.to_string() })?, Elem::str(v)));
        }
        Ok(PackContent { roots: vec![(SHAPES.into(), Neutral::List(shapes)), ("fields".into(), Neutral::Map(BTreeMap::new()))], codec: vec![1] })
    }
    fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        let mut out = String::new();
        for (r, n) in c {
            if let (SHAPES, Neutral::List(l)) = (r.as_str(), n) {
                for (k, v) in l {
                    out.push_str(&format!("{k}={}\n", v.as_str().unwrap_or("")));
                }
            }
        }
        Ok(out.into_bytes())
    }
}

/// A table: the kit's table roots, empty.
struct Table;

impl Pack for Table {
    fn schema(&self) -> Schema {
        Schema { name: "table-k2", roots: k2_mpc_base::table::ROOTS }
    }
    fn decode(&self, _: &[u8]) -> Result<PackContent, Declined> {
        let roots = k2_mpc_base::table::ROOTS
            .iter()
            .map(|(name, kind)| {
                let n = match kind {
                    Kind::MovableList => Neutral::List(Vec::new()),
                    _ => Neutral::Map(BTreeMap::new()),
                };
                (name.to_string(), n)
            })
            .collect();
        Ok(PackContent { roots, codec: vec![1] })
    }
    fn encode(&self, _: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        Ok(Vec::new())
    }
}

struct TestHost;

impl Host for TestHost {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::editor()
    }
    fn now_ms(&self) -> u64 {
        1_700_000_000_000
    }
    fn on_event(&self, _: &AuditEvent) {}
}

struct Fx {
    dir: std::path::PathBuf,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Fx {
    fn new() -> Fx {
        Fx::with_config(|_| {})
    }
    fn with_config(tune: impl FnOnce(&mut k2_mpc_base::session::Config)) -> Fx {
        let dir = std::env::temp_dir().join(format!("k2-live-watch-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        let mut config = k2_mpc_base::session::Config::default();
        config.edit_rate_per_s = 1_000_000;
        config.edit_burst = 1_000_000;
        tune(&mut config);
        let store = Arc::new(DocStore::open(dir.join("docs.redb")).expect("store"));
        Fx { dir, mgr: SessionManager::new(store, Arc::new(TestHost), config) }
    }
    fn open(&self, pack: Arc<dyn Pack>, path: &str, bytes: &[u8]) -> Arc<Session> {
        self.mgr.open(pack, &DocKey { space: "ws".into(), path: path.into() }, Some(bytes)).expect("open")
    }
    fn board(&self, bytes: &str) -> Arc<Session> {
        self.open(Arc::new(Board), "plan.board", bytes.as_bytes())
    }
}

fn join(s: &Session, pid: &str) -> ConnId {
    s.join(Participant::new(pid, pid, "#2f9e8f", ParticipantKind::Person, "tab"), Stay::Held).expect("join").conn
}

fn parse(frames: &[String]) -> Vec<J> {
    frames.iter().map(|f| serde_json::from_str(f).unwrap_or_else(|e| panic!("frame JSON ({e}): {f}"))).collect()
}

fn insert(s: &Session, conn: ConnId, key: &str, after: &str) -> u64 {
    let op = ListOp::Insert { key: Key::new(key).expect("key"), at: Anchor::After(Key::new(after).expect("key")), value: Elem::str("box") };
    s.edit(conn, vec![Edit::List { root: SHAPES.into(), ops: vec![op] }], None).expect("insert").version
}

const START: &str = "a1=box\na2=line\n";

#[test]
fn only_a_keyed_doc_that_isnt_a_table_lists_the_keyed_watch() {
    let f = Fx::new();
    assert_eq!(features(&f.board(START)), vec!["keyed_watch"]);
    assert_eq!(KEYED_WATCH_FEATURE, "keyed_watch");
    let table = f.open(Arc::new(Table), "sheet.csv", b"");
    assert!(features(&table).is_empty(), "a table has its own frames, never the keyed watch");
    let text = f.open(Arc::new(TextFilePack::new("markdown", 4 << 20)), "notes.md", b"one\n");
    assert!(features(&text).is_empty(), "a text doc has the hunk channel, never the keyed watch");
    assert_eq!(features_value(&[KEYED_WATCH_FEATURE]).to_json(), r#"["keyed_watch"]"#);
    assert_eq!(features_value(&[]).to_json(), "[]");
}

#[test]
fn a_watch_sends_a_snapshot_in_pages_then_one_numbered_ops_change_per_change() {
    let f = Fx::new();
    let s = f.board(START);
    let me = join(&s, "person:owner");
    let ada = join(&s, "person:ada");
    let mut w = SocketWatch::new(&features(&s));
    assert!(w.listed() && !w.open());
    assert!(w.pump(&s).is_empty(), "nothing before the client's watch");

    let snap = parse(&w.on_watch(&s, me).expect("watch taken"));
    assert!(w.open());
    assert_eq!(snap[0]["type"], "keyed_snapshot", "{}", snap[0]);
    assert_eq!(snap[0]["seq"], 1);
    let pages = snap[0]["pages"].as_u64().expect("pages") as usize;
    assert!(pages >= 1, "{}", snap[0]);
    assert_eq!(snap.len(), pages + 1, "the head, then its pages: {snap:?}");
    for (i, p) in snap[1..].iter().enumerate() {
        assert_eq!(p["type"], "keyed_page", "{p}");
        assert_eq!(p["seq"], i as u64 + 2, "{p}");
        assert_eq!(p["page"], i as u64 + 1, "{p}");
    }
    assert_eq!(snap[1]["root"], SHAPES);
    assert_eq!(snap[1]["live"], serde_json::json!([["a1", "box"], ["a2", "line"]]), "{}", snap[1]);
    let last = pages as u64 + 1;

    // One change, one frame, the next seq.
    let rev = insert(&s, ada, "a3", "a2");
    let ch = parse(&w.pump(&s));
    assert_eq!(ch.len(), 1, "one frame per change: {ch:?}");
    assert_eq!(ch[0]["type"], "ops_change", "{}", ch[0]);
    assert_eq!(ch[0]["seq"], last + 1, "{}", ch[0]);
    assert_eq!(ch[0]["rev"], rev, "{}", ch[0]);
    assert_eq!(ch[0]["actor"]["id"], "person:ada", "{}", ch[0]);
    assert_eq!(ch[0]["ops"][0]["op"], "insert", "{}", ch[0]);
    assert_eq!(ch[0]["ops"][0]["key"], "a3", "{}", ch[0]);
    assert!(w.pump(&s).is_empty(), "nothing more queued");

    // A second watch (the client saw a gap): a fresh snapshot, numbered on.
    let again = parse(&w.on_watch(&s, me).expect("watch again"));
    assert_eq!(again[0]["type"], "keyed_snapshot");
    assert_eq!(again[0]["seq"], last + 2, "numbering goes on per connection: {}", again[0]);
    assert_eq!(again[0]["upto"], rev, "{}", again[0]);
    assert_eq!(again[1]["live"], serde_json::json!([["a1", "box"], ["a2", "line"], ["a3", "box"]]), "{}", again[1]);
}

#[test]
fn the_frames_are_the_kits_own_byte_for_byte() {
    let f = Fx::new();
    let s = f.board(START);
    let me = join(&s, "person:owner");
    let other = join(&s, "person:bob");
    let mut w = SocketWatch::new(&[KEYED_WATCH_FEATURE]);
    let ours = w.on_watch(&s, me).expect("watch");
    let mut kit = s.watch_keyed(other, &WatchOptions::new(&[KEYED_WATCH_FEATURE])).expect("watch").expect("watchable");
    let theirs: Vec<String> = s.watch_snapshot(&mut kit).expect("snapshot").iter().map(|f| f.to_json()).collect();
    assert_eq!(ours, theirs);
}

#[test]
fn a_socket_that_didnt_list_it_or_a_text_doc_takes_no_watch() {
    let f = Fx::new();
    let s = f.board(START);
    let me = join(&s, "person:owner");
    let mut w = SocketWatch::new(&[]);
    assert!(!w.listed());
    let e = w.on_watch(&s, me).expect_err("not listed");
    assert!(e.0.contains("welcome.features"), "{e:?}");
    assert!(w.on_report().is_err());
    assert!(!w.open());

    let text = f.open(Arc::new(TextFilePack::new("markdown", 4 << 20)), "notes.md", b"one\n");
    let tc = join(&text, "person:owner");
    let mut w = SocketWatch::new(&[KEYED_WATCH_FEATURE]);
    let e = w.on_watch(&text, tc).expect_err("a text doc can't be watched by key");
    assert!(e.0.contains("can't be watched"), "{e:?}");
    assert!(!w.open());
}

#[test]
fn a_snapshot_over_the_frame_cap_goes_out_as_watch_refused_once() {
    let f = Fx::new();
    let big = format!("a1={}\n", "x".repeat(8 << 10));
    let s = f.board(&big);
    let me = join(&s, "person:owner");
    let ada = join(&s, "person:ada");
    let mut w = SocketWatch::with_options(WatchOptions::new(&[KEYED_WATCH_FEATURE]).page_bytes(1 << 10).frame_max(2 << 10));
    let out = parse(&w.on_watch(&s, me).expect("watch taken"));
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["type"], "watch_refused", "{}", out[0]);
    assert_eq!(out[0]["error"]["id"], "MPC-E401", "{}", out[0]);
    assert!(out[0].get("seq").is_none(), "unnumbered: {}", out[0]);
    // No retry on its own: a change goes nowhere until the client asks again.
    insert(&s, ada, "a2", "a1");
    assert!(w.pump(&s).is_empty(), "nothing after a refusal until the client's next watch");
}

/// Kit v0.19.0: a claim's end names its elements by address as they were,
/// and K2's roster encoder sends them as `claim_ended`'s `keys`, after
/// `at_rev`, only when there are some (the kit's WIRE.md and golden
/// `roster_events_claims_elems.json`). A claim on lines has none, so its
/// bytes are as before (`roster_events.json`, `live_wire_golden_tests`).
#[test]
fn a_claim_on_elements_ends_with_its_keys_on_the_roster() {
    let f = Fx::with_config(|c| c.roster_interval_ms = 0);
    let s = f.board(START);
    let me = join(&s, "person:owner");
    let bot = s.join(Participant::new("agent:bot", "Bot", "#0891b2", ParticipantKind::Agent, "cli"), Stay::Held).expect("join").conn;
    let roster = s.subscribe_roster(me).expect("roster");
    let keys = [Key::new("a2").expect("key"), Key::new("a1").expect("key")];
    let id = s.claim_elems(bot, SHAPES, &keys, None, None, true).expect("claim").id;
    s.release_claim(bot, Some(id)).expect("release");
    s.flush_roster().expect("roster flush");
    let ended: Vec<String> = roster
        .try_iter()
        .filter_map(|e| super::super::live_wire::roster_event(&e))
        .map(|v| v.to_json())
        .filter(|j| j.starts_with(r#"{"kind":"claim_ended""#))
        .collect();
    assert_eq!(ended.len(), 1, "one end: {ended:?}");
    let e: J = serde_json::from_str(&ended[0]).expect("event JSON");
    assert_eq!(e["how"], "released", "{e}");
    assert_eq!(e["who"], "agent:bot", "{e}");
    assert!(
        ended[0].ends_with(r#","keys":["a1","a2"]}"#),
        "the claim's elements by address, in key order, last (after `at_rev`), as the kit's golden: {}",
        ended[0]
    );
}
