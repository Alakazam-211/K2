//! The pack verb on a change: the kit records the
//! id of the pack verb that made a change (`shape move`), on the change record, its events and
//! history, and none for a raw write, a kit tool, an outside save, an undo, a redo, a revert or
//! a restore; `via` stays the client. And an agent's presence from its verbs (Item 3):
//! a verb that changes elements sets the agent's focus to the container of the last element it
//! touched and its selection to what it touched; a person's presence is never moved. Public API
//! only.
//!
//! The doc is deck-shaped, as `tests/contract_presence.rs`'s: `slides` and `shapes`
//! (MovableLists) and `fields` (a Map of `<shape>\x1f<field>`, composite on `shapes`), whose
//! pack names a shape's slide as its container (`s259-3` is on `s259`) and has structured verbs
//! of its own.
//!
//! Planted faults caught by name (`k2_mpc_base::differential::ContractMutant`): each
//! `plant_*_is_caught` row runs its check under the fault and requires it to fail where it says.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use k2_mpc_base::differential::{with_contract_mutant, ContractMutant};
use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{ListOp, MapOp};
use k2_mpc_base::parity::PersonAction;
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    AuditEvent, ChangeEvent, Config, ConnId, Declined, EditOptions, ElemFocus, ElemSelection, Host,
    LogItem, Pack, PackContent, PackError, Participant, ParticipantKind, PurgeRequest, PurgeScope,
    Rights, Session, SessionManager, Stay, TextFilePack, UndoRequest,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::{
    EditOutput, PackEdit, PackVerb, StructEdit, StructOutput, ToolError, VerbCtx,
};
use k2_mpc_base::{Edit, Key, Value};

const SLIDES: &str = "slides";
const SHAPES: &str = "shapes";
const FIELDS: &str = "fields";

// ---------- a deck-shaped pack with verbs of its own ----------

/// A deck-shaped keyed doc. The file is one line a slide (`s256`) followed by its shapes
/// (`  s256-2`); a shape's `geom` field starts at `0`.
struct Deck;

const DECK: Schema = Schema {
    name: "deck-verbs",
    roots: &[
        (SLIDES, Kind::MovableList),
        (SHAPES, Kind::MovableList),
        (FIELDS, Kind::Map),
    ],
};

impl Pack for Deck {
    fn schema(&self) -> Schema {
        DECK
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        &[(FIELDS, SHAPES)]
    }
    /// A shape `s<slide>-<n>` sits on slide `s<slide>`; a slide is its own container.
    fn container_of(&self, root: &str, key: &Key) -> Option<(String, Key)> {
        match root {
            SHAPES => {
                let (slide, _) = key.as_str().split_once('-')?;
                Some((SLIDES.to_owned(), Key::new(slide).ok()?))
            }
            SLIDES => Some((SLIDES.to_owned(), key.clone())),
            _ => None,
        }
    }
    fn verbs(&self) -> &'static [PackVerb] {
        DECK_VERBS
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let (mut slides, mut shapes, mut fields) = (Vec::new(), Vec::new(), BTreeMap::new());
        for line in std::str::from_utf8(b).unwrap().lines() {
            let k = Key::new(line.trim()).unwrap();
            if line.starts_with(' ') {
                fields.insert(format!("{k}\u{1f}geom"), Value::Str("0".into()));
                shapes.push((k, Value::Null));
            } else {
                slides.push((k, Value::Null));
            }
        }
        Ok(PackContent {
            roots: vec![
                (SLIDES.into(), Neutral::List(slides)),
                (SHAPES.into(), Neutral::List(shapes)),
                (FIELDS.into(), Neutral::Map(fields)),
            ],
            codec: Vec::new(),
        })
    }
    fn encode(&self, _: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        Ok(Vec::new())
    }
}

/// The sample deck's four slides in presentation order 256, 257, 258, 259, with shapes.
const SAMPLE: &[u8] =
    b"s256\n  s256-2\n  s256-3\ns257\n  s257-7\ns258\n  s258-2\ns259\n  s259-2\n  s259-3\n";

const S: &[PersonAction] = &[PersonAction::Format];

const DECK_VERBS: &[PackVerb] = &[
    PackVerb::structured_edit(
        "shape move",
        "shape move <shape>...",
        S,
        &[0, 2],
        shape_move,
    )
    .positions(&[]),
    PackVerb::structured_edit(
        "shape delete",
        "shape delete <shape>...",
        S,
        &[0, 2],
        shape_delete,
    )
    .positions(&[]),
];

fn keys_of(args: &[String]) -> Result<Vec<Key>, ToolError> {
    if args.is_empty() {
        return Err(ToolError::usage("name at least one shape"));
    }
    args.iter()
        .map(|a| Key::new(a).map_err(|_| ToolError::usage(format!("{a:?} is not a key"))))
        .collect()
}

/// `shape move <shape>...`: each shape's `geom` goes one step on, in the order named.
fn shape_move(args: &[String]) -> Result<StructEdit, ToolError> {
    let keys = keys_of(args)?;
    Ok(Box::new(move |ctx: &VerbCtx<'_>| {
        let fields = ctx.view().map(FIELDS).cloned().unwrap_or_default();
        let ops = keys
            .iter()
            .map(|k| {
                let at = format!("{k}\u{1f}geom");
                let n = match fields.get(&at) {
                    Some(Value::Str(s)) => s.parse::<u64>().unwrap_or(0),
                    _ => 0,
                };
                MapOp::Set {
                    key: at,
                    value: Value::Str((n + 1).to_string()),
                }
            })
            .collect();
        Ok(StructOutput::new(
            vec![Edit::Map {
                root: FIELDS.into(),
                ops,
            }],
            Vec::new(),
            0,
        ))
    }))
}

/// `shape delete <shape>...`: the shapes go, in the order named.
fn shape_delete(args: &[String]) -> Result<StructEdit, ToolError> {
    let keys = keys_of(args)?;
    Ok(Box::new(move |_: &VerbCtx<'_>| {
        Ok(StructOutput::new(
            vec![Edit::List {
                root: SHAPES.into(),
                ops: keys
                    .iter()
                    .map(|k| ListOp::Delete { key: k.clone() })
                    .collect(),
            }],
            Vec::new(),
            0,
        ))
    }))
}

// ---------- a text pack with one verb of its own ----------

const MD: TextFilePack = TextFilePack::new("markdown", 4 << 20);
const ROOT: &str = TextFilePack::ROOT;

struct Shout;

impl Pack for Shout {
    fn schema(&self) -> Schema {
        MD.schema()
    }
    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        MD.decode(bytes)
    }
    fn encode(&self, content: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        MD.encode(content, codec)
    }
    fn normalize_input<'a>(&self, root: &str, text: &'a str) -> Cow<'a, str> {
        MD.normalize_input(root, text)
    }
    fn verbs(&self) -> &'static [PackVerb] {
        SHOUT_VERBS
    }
}

/// `shout`: uppercase the doc.
fn shout(args: &[String]) -> Result<PackEdit, ToolError> {
    if !args.is_empty() {
        return Err(ToolError::unsupported(args.join(" "), "shout"));
    }
    Ok(Box::new(|_, text, _| {
        Ok(EditOutput::new(Some(text.to_uppercase()), Vec::new(), 0))
    }))
}

const SHOUT_VERBS: &[PackVerb] = &[PackVerb::edit("shout", "shout", S, &[0], shout).positions(&[])];

// ---------- the host and the fixture ----------

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
            .unwrap_or(Rights::full())
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

struct Fx {
    _dir: TmpDir,
    ws: PathBuf,
    host: Arc<TestHost>,
    mgr: SessionManager,
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-verbs-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let ws = dir.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let ws = std::fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(100_000),
        rights: Mutex::new(HashMap::new()),
    });
    host.rights
        .lock()
        .unwrap()
        .insert("own".into(), Rights::full().with_owner(true));
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host.clone(), c);
    Fx {
        _dir: TmpDir(dir),
        ws,
        host,
        mgr,
    }
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

impl Fx {
    fn deck(&self) -> Arc<Session> {
        self.mgr
            .open(Arc::new(Deck), &key("sample.deck"), Some(SAMPLE))
            .unwrap()
    }
    fn reopen_deck(&self) -> Arc<Session> {
        self.mgr.close(&key("sample.deck"), "test").unwrap();
        self.mgr
            .open(Arc::new(Deck), &key("sample.deck"), None)
            .unwrap()
    }
    fn text(&self, name: &str, body: &str) -> Arc<Session> {
        let p = self.ws.join(name);
        if !p.exists() {
            std::fs::write(&p, body).unwrap();
        }
        self.mgr
            .open_file(Arc::new(Shout), &key(name), &p, &self.ws)
            .unwrap()
    }
    fn tick(&self) {
        self.host.now.fetch_add(300, Ordering::SeqCst);
        self.mgr.flush_rosters();
    }
}

fn person(pid: &str) -> Participant {
    Participant::new(pid, pid, "#2f9e8f", ParticipantKind::Person, "tab")
}

fn agent(pid: &str) -> Participant {
    Participant::new(pid, "Atlas", "#6d5bd0", ParticipantKind::Agent, "cli")
}

fn join(s: &Session, p: Participant) -> ConnId {
    s.join(p, Stay::Held).unwrap().conn
}

fn k(s: &str) -> Key {
    Key::new(s).unwrap()
}

fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn drain(rx: &Receiver<ChangeEvent>) -> Vec<ChangeEvent> {
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

/// Run a deck verb as `c`, guarded by the doc's version now (an agent's structured edit needs a
/// guard), and require it to change the doc. Returns the new version.
fn verb(s: &Session, c: ConnId, words: &[&str]) -> u64 {
    let v = s.status().unwrap().version;
    let mut a = argv(words);
    a.extend(["--if-version".to_owned(), v.to_string()]);
    let out = s.run_tool(c, SHAPES, &a, b"").unwrap();
    assert_eq!(out.exit, 0, "{words:?}: {}", out.stderr_summary);
    assert!(out.changed.is_some(), "{words:?} changed nothing");
    out.version
}

/// Every change in the log as `(seq, verb)`, oldest first.
fn log_verbs(s: &Session, c: ConnId) -> Vec<(u64, Option<String>)> {
    let mut v: Vec<(u64, Option<String>)> = s
        .log(c, &Default::default())
        .unwrap()
        .into_iter()
        .filter_map(|i| match i {
            LogItem::Change { info, .. } => Some((info.seq, info.verb)),
            _ => None,
        })
        .collect();
    v.sort();
    v
}

/// The verb of change `seq` in the log, `None` when it has none; panics when the log hasn't it.
fn logged(log: &[(u64, Option<String>)], seq: u64) -> Option<&str> {
    log.iter()
        .find(|(s, _)| *s == seq)
        .unwrap_or_else(|| panic!("change {seq} isn't in the log: {log:?}"))
        .1
        .as_deref()
}

// ---------- the verb on the change ----------

/// The check behind the deck rows: an agent's `shape move` records its verb on every event of
/// its change and in the log, and a reload reads it back; `via` stays the client. A raw write, an undo, a redo, a revert and a restore record none. `Err` names the
/// first place the verb isn't what it should be.
fn deck_verbs_recorded() -> Result<(), String> {
    let f = fx("deck");
    let s = f.deck();
    let own = join(&s, person("own"));
    let bot = join(&s, agent("atlas"));
    let rx = s.subscribe(own).unwrap();
    let moved = verb(&s, bot, &["shape", "move", "s256-2", "s259-3"]);
    // a raw write (ops by key): no verb
    let raw = s
        .edit_ops(
            bot,
            vec![Edit::Map {
                root: FIELDS.into(),
                ops: vec![MapOp::Set {
                    key: "s257-7\u{1f}geom".into(),
                    value: Value::Str("9".into()),
                }],
            }],
            &EditOptions::new().if_version(moved),
        )
        .unwrap()
        .version;
    // a named version to restore, then undo, redo, revert and restore: none of them is a verb
    let vid = s.name_version(own, "before", None).unwrap();
    let deleted = verb(&s, bot, &["shape", "delete", "s258-2"]);
    s.undo_with(bot, &UndoRequest::steps(1)).unwrap();
    let undone = s.status().unwrap().version;
    s.redo_with(bot, &UndoRequest::steps(1)).unwrap();
    let redone = s.status().unwrap().version;
    s.revert(own, moved, false).unwrap();
    let reverted = s.status().unwrap().version;
    s.restore(own, vid).unwrap();
    let restored = s.status().unwrap().version;
    let want: Vec<(u64, Option<&str>)> = vec![
        (moved, Some("shape move")),
        (raw, None),
        (deleted, Some("shape delete")),
        (undone, None),
        (redone, None),
        (reverted, None),
        (restored, None),
    ];
    // the events: every event of a change carries its verb; via is still the client
    let events = drain(&rx);
    for (seq, v) in &want {
        let of: Vec<&ChangeEvent> = events.iter().filter(|e| e.version == *seq).collect();
        if of.is_empty() {
            return Err(format!("the events: none for change {seq}"));
        }
        for e in of {
            if e.verb.as_deref() != *v {
                return Err(format!(
                    "the events: change {seq} ({:?}) has verb {:?}, want {v:?}",
                    e.kind, e.verb
                ));
            }
            if e.pid == "atlas" && e.via != "cli" {
                return Err(format!("the events: via {:?} isn't the client", e.via));
            }
        }
    }
    // the log, and the log after a reload (a keyed doc has no live window: `events_since` is a
    // text doc's, so the text row reads it)
    let log = log_verbs(&s, own);
    for (seq, v) in &want {
        if logged(&log, *seq) != *v {
            return Err(format!(
                "the log: change {seq} has verb {:?}, want {v:?}",
                logged(&log, *seq)
            ));
        }
    }
    drop(s);
    let s = f.reopen_deck();
    let own = join(&s, person("own"));
    let log = log_verbs(&s, own);
    for (seq, v) in &want {
        if logged(&log, *seq) != *v {
            return Err(format!(
                "after a reload, the log: change {seq} has verb {:?}, want {v:?}",
                logged(&log, *seq)
            ));
        }
    }
    Ok(())
}

#[test]
fn a_pack_verb_is_recorded_on_its_change_and_nothing_else_is() {
    deck_verbs_recorded().unwrap();
}

/// The planted fault `VerbNotRecorded`: the change is stored without its verb, so the events
/// still carry it and the log (read from the store) doesn't.
#[test]
fn plant_verb_not_recorded_is_caught() {
    let e = with_contract_mutant(ContractMutant::VerbNotRecorded, deck_verbs_recorded)
        .expect_err("the check passes with the verb never stored");
    assert!(e.starts_with("the log: change"), "{e}");
}

/// A text pack's own verb records its id too, on its event, the live window (`events_since`,
/// read back from the store) and the log; a kit tool, a whole-text write and an outside save
/// record none.
#[test]
fn a_text_pack_verb_is_recorded_and_a_kit_tool_is_not() {
    let f = fx("text");
    let s = f.text("a.md", "one\n");
    let own = join(&s, person("own"));
    let bot = join(&s, agent("atlas"));
    let rx = s.subscribe(own).unwrap();
    let v0 = s.status().unwrap().version;
    let shouted = s.run_tool(bot, ROOT, &argv(&["shout"]), b"").unwrap();
    assert!(shouted.changed.is_some());
    let appended = s.run_tool(bot, ROOT, &argv(&["append"]), b"two\n").unwrap();
    assert!(appended.changed.is_some());
    s.replace_text(bot, ROOT, "ONE\ntwo\nthree\n").unwrap();
    let written = s.status().unwrap().version;
    std::fs::write(f.ws.join("a.md"), "ONE\ntwo\nthree\nfour\n").unwrap();
    s.file_changed().unwrap();
    let saved = s.status().unwrap().version;
    assert!(saved > written, "the outside save made a change");
    let want = [
        (shouted.version, Some("shout")),
        (appended.version, None),
        (written, None),
        (saved, None),
    ];
    let events = drain(&rx);
    for (seq, v) in want {
        let e = events
            .iter()
            .find(|e| e.version == seq)
            .unwrap_or_else(|| panic!("no event for change {seq}"));
        assert_eq!(e.verb.as_deref(), v, "change {seq}'s event ({:?})", e.kind);
    }
    let window = s.events_since(own, v0).unwrap();
    for (seq, v) in want {
        let e = window
            .iter()
            .find(|e| e.version == seq)
            .unwrap_or_else(|| panic!("change {seq} isn't in the live window"));
        assert_eq!(e.verb.as_deref(), v, "change {seq} in the live window");
    }
    let log = log_verbs(&s, own);
    for (seq, v) in want {
        assert_eq!(logged(&log, seq), v, "change {seq} in the log");
    }
}

/// A participant's erasure clears the verbs of their changes and of those an agent made on
/// their behalf, as it clears turns; another's stay.
#[test]
fn erasure_clears_the_verbs_of_the_erased_participants_changes() {
    let f = fx("erase");
    let s = f.deck();
    let own = join(&s, person("own"));
    let atlas = join(&s, agent("atlas"));
    let nova = join(
        &s,
        Participant::new("nova", "Nova", "#b0577a", ParticipantKind::Agent, "cli")
            .on_behalf_of("ada"),
    );
    let a = verb(&s, atlas, &["shape", "move", "s256-2"]);
    let n = verb(&s, nova, &["shape", "move", "s257-7"]);
    let with = |s: &Session| -> Vec<u64> {
        log_verbs(s, own)
            .into_iter()
            .filter(|(_, v)| v.is_some())
            .map(|(seq, _)| seq)
            .collect()
    };
    assert_eq!(with(&s), [a, n], "both verbs are logged before");
    let erase = |pid: &str, ps: &str| {
        s.purge(
            own,
            PurgeRequest::new(PurgeScope::Participant {
                pid: pid.into(),
                pseudonym: ps.into(),
            }),
        )
        .unwrap();
    };
    erase("atlas", "erased-1");
    assert_eq!(with(&s), [n], "after erasing atlas, nova's verb stays");
    erase("ada", "erased-2");
    assert!(
        with(&s).is_empty(),
        "after erasing ada (nova wrote on her behalf), none"
    );
}

// ---------- an agent's presence from its verbs ----------

/// A participant's focus (`root:key`) and selection (`root:key` each).
type Seen = (Option<String>, Option<Vec<String>>);

/// `pid`'s focus and selection as the roster has them.
fn presence(s: &Session, c: ConnId, pid: &str) -> Seen {
    let e = s
        .roster(c)
        .unwrap()
        .into_iter()
        .find(|e| e.participant.pid == pid)
        .unwrap_or_else(|| panic!("{pid} isn't in the roster"));
    (
        e.focus.map(|f| format!("{}:{}", f.root, f.key)),
        e.selection
            .map(|x| x.keys.iter().map(|k| format!("{}:{k}", x.root)).collect()),
    )
}

fn want(focus: &str, selection: &[&str]) -> Seen {
    (
        Some(focus.to_owned()),
        (!selection.is_empty()).then(|| selection.iter().map(|x| x.to_string()).collect()),
    )
}

/// The check behind the agent rows: each verb an agent runs moves its focus to the container of
/// the last element it touched (the last container touched wins) and its selection to what it
/// touched, whatever they were; a deleted element can't be selected; a raw write moves nothing.
/// `Err` names the first step whose presence is wrong.
fn agent_verbs_move_presence() -> Result<(), String> {
    let f = fx("agent");
    let s = f.deck();
    let own = join(&s, person("own"));
    let bot = join(&s, agent("atlas"));
    s.set_focus(bot, Some(ElemFocus::new(SLIDES, k("s257"))))
        .unwrap();
    s.set_selection(bot, Some(ElemSelection::new(SHAPES, vec![k("s257-7")])))
        .unwrap();
    let steps: [(&str, &[&str], Seen); 3] = [
        (
            "shape move s256-2 s259-3",
            &["shape", "move", "s256-2", "s259-3"],
            want("slides:s259", &["shapes:s256-2", "shapes:s259-3"]),
        ),
        (
            "shape move s259-3 s256-2",
            &["shape", "move", "s259-3", "s256-2"],
            want("slides:s256", &["shapes:s259-3", "shapes:s256-2"]),
        ),
        (
            "shape delete s258-2",
            &["shape", "delete", "s258-2"],
            (Some("slides:s258".to_owned()), None),
        ),
    ];
    for (what, words, wanted) in steps {
        verb(&s, bot, words);
        f.tick();
        let got = presence(&s, own, "atlas");
        if got != wanted {
            return Err(format!(
                "after the agent's {what}: presence {got:?}, want {wanted:?}"
            ));
        }
    }
    // a raw write is no verb: the presence stays where the last verb put it
    let v = s.status().unwrap().version;
    s.edit_ops(
        bot,
        vec![Edit::Map {
            root: FIELDS.into(),
            ops: vec![MapOp::Set {
                key: "s257-7\u{1f}geom".into(),
                value: Value::Str("5".into()),
            }],
        }],
        &EditOptions::new().if_version(v),
    )
    .unwrap();
    f.tick();
    let got = presence(&s, own, "atlas");
    let wanted: Seen = (Some("slides:s258".to_owned()), None);
    if got != wanted {
        return Err(format!(
            "after the agent's raw write: presence {got:?}, want {wanted:?}"
        ));
    }
    Ok(())
}

#[test]
fn an_agents_verb_moves_its_focus_and_selection() {
    agent_verbs_move_presence().unwrap();
}

/// The check behind the person row: a person's own verb leaves their focus and selection where
/// they put them. `Err` says how they moved.
fn person_verb_leaves_presence() -> Result<(), String> {
    let f = fx("person");
    let s = f.deck();
    let ada = join(&s, person("ada"));
    s.set_focus(ada, Some(ElemFocus::new(SLIDES, k("s256"))))
        .unwrap();
    s.set_selection(ada, Some(ElemSelection::new(SHAPES, vec![k("s256-3")])))
        .unwrap();
    verb(&s, ada, &["shape", "move", "s259-2"]);
    f.tick();
    let got = presence(&s, ada, "ada");
    let wanted = want("slides:s256", &["shapes:s256-3"]);
    if got != wanted {
        return Err(format!(
            "a person's presence moved: {got:?}, want {wanted:?} (as they set it)"
        ));
    }
    Ok(())
}

#[test]
fn a_persons_verb_leaves_their_presence() {
    person_verb_leaves_presence().unwrap();
}

/// The planted fault `VerbMovesPersonPresence`: the person's presence follows their verb.
#[test]
fn plant_verb_moves_a_persons_presence_is_caught() {
    let e = with_contract_mutant(
        ContractMutant::VerbMovesPersonPresence,
        person_verb_leaves_presence,
    )
    .expect_err("the check passes with a person's presence moved by their verb");
    assert!(e.starts_with("a person's presence moved"), "{e}");
    // the agent rows hold under it: the fault moves people, not agents
    with_contract_mutant(
        ContractMutant::VerbMovesPersonPresence,
        agent_verbs_move_presence,
    )
    .unwrap();
}
