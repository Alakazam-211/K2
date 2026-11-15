//! A participant's view address (the view they are looking at, by address) and time position
//! (a playhead: a decimal position in the pack's unit, playing or paused, its rate and what it
//! is in), on a keyed doc. People set their own; a pack's verb sets its agent's
//! (`StructOutput::viewing`, `StructOutput::time_position`); a host sets one for any connection.
//! Both reach other viewers through the roster, are absent until set, and are checked. Public API
//! only.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::MapOp;
use k2_mpc_base::parity::PersonAction;
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Declined, Host, Pack, PackContent, PackError, Participant,
    ParticipantKind, PlayState, Rights, RosterEntry, RosterEvent, Session, SessionError,
    SessionManager, Stay, TimePosition, PRESENCE_ADDRESS_MAX,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::{PackVerb, StructEdit, StructOutput, ToolError, ToolOptions, VerbCtx};
use k2_mpc_base::view_index::{Src, UnitFields, ViewSchema};
use k2_mpc_base::{Edit, Key, Value};

// ---------- a timeline-shaped toy: tracks hold clips ----------

const TRACKS: &str = "tracks";
const CLIPS: &str = "clips";
const CLIP_F: &str = "clip_f";
const SEP: char = '\u{1f}';

const SCHEMA: Schema = Schema {
    name: "toy-timeline",
    roots: &[
        (TRACKS, Kind::MovableList),
        (CLIPS, Kind::MovableList),
        (CLIP_F, Kind::Map),
    ],
};

/// Tracks hold clips; positions count in ticks.
const VIEW: ViewSchema = ViewSchema {
    containers: TRACKS,
    units: CLIPS,
    unit_container_field: "track",
    native_unit: "tick",
    unit_fields: UnitFields {
        kind: Src::ListValue,
        ..UnitFields::NONE
    },
    time_unit: Some("tick"),
    ..ViewSchema::NONE
};

struct Toy;

const S: &[PersonAction] = &[PersonAction::Format];

const VERBS: &[PackVerb] = &[
    PackVerb::structured_read("clip look", "clip look <address>", S, &[0, 2], clip_look)
        .positions(&[]),
    PackVerb::structured_edit(
        "clip play",
        "clip play <clip> <at> <playing|paused>",
        S,
        &[0, 2],
        clip_play,
    )
    .positions(&[]),
];

/// `clip look <address>`: a still of the view at `address` (it reads nothing): the agent is
/// looking at it.
fn clip_look(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = args
        .first()
        .cloned()
        .ok_or_else(|| ToolError::usage("name a view"))?;
    Ok(Box::new(move |_: &VerbCtx<'_>| {
        Ok(StructOutput::new(Vec::new(), b"looked\n".to_vec(), 0).viewing(a.clone()))
    }))
}

/// `clip play <clip> <at> <state>`: the clip's `playhead` field is `at`, and the agent's
/// playhead is there, on the clip's track `@tr:v1`, at half speed.
fn clip_play(args: &[String]) -> Result<StructEdit, ToolError> {
    let [clip, at, state] = args else {
        return Err(ToolError::usage("clip play <clip> <at> <state>"));
    };
    let clip = clip.clone();
    let at = at.clone();
    let state = PlayState::parse(state).ok_or_else(|| ToolError::usage("playing or paused"))?;
    Ok(Box::new(move |_: &VerbCtx<'_>| {
        let t = TimePosition::new(&at, state)
            .and_then(|t| t.within("@tr:v1"))
            .and_then(|t| t.rate("1/2"))
            .map_err(|e| ToolError::usage(e.to_string()))?;
        Ok(StructOutput::new(
            vec![Edit::Map {
                root: CLIP_F.into(),
                ops: vec![MapOp::Set {
                    key: format!("{clip}{SEP}playhead"),
                    value: Value::str(at.clone()),
                }],
            }],
            Vec::new(),
            0,
        )
        .time_position(t))
    }))
}

impl Pack for Toy {
    fn schema(&self) -> Schema {
        SCHEMA
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        &[(CLIP_F, CLIPS)]
    }
    fn view_schema(&self) -> Option<ViewSchema> {
        Some(VIEW)
    }
    fn verbs(&self) -> &'static [PackVerb] {
        VERBS
    }
    fn decode(&self, _: &[u8]) -> Result<PackContent, Declined> {
        let k = |s: &str| Key::new(s).unwrap();
        let mut f = BTreeMap::new();
        for c in ["c1", "c2"] {
            f.insert(format!("{c}{SEP}track"), Value::str("v1"));
        }
        Ok(PackContent {
            roots: vec![
                (TRACKS.into(), Neutral::List(vec![(k("v1"), Value::Null)])),
                (
                    CLIPS.into(),
                    Neutral::List(vec![
                        (k("c1"), Value::str("clip")),
                        (k("c2"), Value::str("clip")),
                    ]),
                ),
                (CLIP_F.into(), Neutral::Map(f)),
            ],
            codec: Vec::new(),
        })
    }
    fn encode(&self, _: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        Ok(Vec::new())
    }
}

// ---------- the fixture ----------

struct TestHost {
    now: AtomicU64,
}

impl Host for TestHost {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

struct TmpDir(PathBuf);
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    _dir: TmpDir,
    host: Arc<TestHost>,
    mgr: SessionManager,
    s: Arc<Session>,
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-view-time-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_700_000_000_000),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host.clone(), c);
    let s = mgr
        .open(
            Arc::new(Toy),
            &DocKey {
                space: "ws".into(),
                path: "cut.toy".into(),
            },
            Some(b"toy"),
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
    fn join(&self, pid: &str, kind: ParticipantKind) -> ConnId {
        self.s
            .join(
                Participant::new(pid, pid, "#2f9e8f", kind, "tab"),
                Stay::Held,
            )
            .unwrap()
            .conn
    }
    fn tick(&self) {
        self.host.now.fetch_add(300, Ordering::SeqCst);
        self.mgr.flush_rosters();
    }
    fn now(&self) -> u64 {
        self.host.now.load(Ordering::SeqCst)
    }
    fn entry(&self, viewer: ConnId, pid: &str) -> RosterEntry {
        self.s
            .roster(viewer)
            .unwrap()
            .into_iter()
            .find(|e| e.participant.pid == pid)
            .unwrap_or_else(|| panic!("no roster row for {pid}"))
    }
    fn verb(&self, c: ConnId, words: &[&str]) -> k2_mpc_base::tools::ToolOutput {
        let mut a: Vec<String> = words.iter().map(|w| w.to_string()).collect();
        if words.first() == Some(&"clip") && words.get(1) == Some(&"play") {
            let v = self.s.status().unwrap().version;
            a.extend(["--if-version".to_owned(), v.to_string()]);
        }
        self.s
            .run_tool_with(c, CLIPS, &a, b"", &ToolOptions::new())
            .unwrap_or_else(|e| panic!("{words:?}: {e}"))
    }
}

fn drain<T>(rx: &Receiver<T>) -> Vec<T> {
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

fn need(ok: bool, what: impl FnOnce() -> String) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(what())
    }
}

fn playing(at: &str) -> TimePosition {
    TimePosition::new(at, PlayState::Playing)
        .unwrap()
        .within("@tr:v1")
        .unwrap()
        .rate("1/2")
        .unwrap()
}

// ---------- the rows ----------

/// A person sets a view address and a time position, gets them back as stored (the time stamped
/// with the host clock), and their roster row and their connection's device carry both.
fn row_a_person_sets_and_reads_back() -> Result<(), String> {
    let f = fx("person");
    let ada = f.join("ada", ParticipantKind::Person);
    let got = f.s.set_view_address(ada, Some("@tr:v1")).unwrap();
    need(got.as_deref() == Some("@tr:v1"), || {
        format!("set_view_address gave {got:?}")
    })?;
    let at_ms = f.now();
    let t =
        f.s.set_time_position(ada, Some(playing("508032000000")))
            .unwrap();
    let t = t.ok_or("set_time_position gave none")?;
    need(
        (t.at(), t.state(), t.at_ms(), t.in_address(), t.rate_text())
            == (
                "508032000000",
                PlayState::Playing,
                at_ms,
                Some("@tr:v1"),
                Some("1/2"),
            ),
        || format!("set_time_position gave {t:?}"),
    )?;
    let e = f.entry(ada, "ada");
    need(e.view_address.as_deref() == Some("@tr:v1"), || {
        format!("the roster row's view address: {:?}", e.view_address)
    })?;
    need(e.time.as_ref() == Some(&t), || {
        format!("the roster row's time: {:?}", e.time)
    })?;
    let d = e.devices.first().ok_or("no device")?;
    need(
        d.view_address.as_deref() == Some("@tr:v1") && d.time.as_ref() == Some(&t),
        || format!("the device: {:?} {:?}", d.view_address, d.time),
    )?;
    // cleared
    f.s.set_view_address(ada, None).unwrap();
    f.s.set_time_position(ada, None).unwrap();
    let e = f.entry(ada, "ada");
    need(e.view_address.is_none() && e.time.is_none(), || {
        format!("cleared: {:?} {:?}", e.view_address, e.time)
    })
}

#[test]
fn a_person_sets_and_reads_back_a_view_address_and_a_time_position() {
    row_a_person_sets_and_reads_back().unwrap();
}

/// An agent's verbs set its view address (a read verb's still) and its playhead (an edit verb),
/// stamped when the verb ran; the same verbs leave a person's as they were; a host sets an
/// agent's directly too (it rendered a still for it).
fn row_an_agent_verb_sets_them() -> Result<(), String> {
    let f = fx("agent");
    let bot = f.join("atlas", ParticipantKind::Agent);
    let ada = f.join("ada", ParticipantKind::Person);
    let out = f.verb(bot, &["clip", "look", "@clip:c2"]);
    need(out.exit == 0, || format!("clip look: {out:?}"))?;
    let e = f.entry(ada, "atlas");
    need(e.view_address.as_deref() == Some("@clip:c2"), || {
        format!(
            "the agent's view address after its read verb: {:?}",
            e.view_address
        )
    })?;
    f.host.now.fetch_add(50, Ordering::SeqCst);
    let at_ms = f.now();
    let out = f.verb(bot, &["clip", "play", "c1", "1000", "paused"]);
    need(out.exit == 0 && out.changed.is_some(), || {
        format!("clip play: {out:?}")
    })?;
    let t = f
        .entry(ada, "atlas")
        .time
        .ok_or("no time after the agent's verb")?;
    need(
        (t.at(), t.state(), t.at_ms(), t.in_address(), t.rate_text())
            == (
                "1000",
                PlayState::Paused,
                at_ms,
                Some("@tr:v1"),
                Some("1/2"),
            ),
        || format!("the agent's time: {t:?}"),
    )?;
    // a person running the same verbs keeps their own
    f.verb(ada, &["clip", "look", "@clip:c1"]);
    f.verb(ada, &["clip", "play", "c2", "5", "playing"]);
    let e = f.entry(ada, "ada");
    need(e.view_address.is_none() && e.time.is_none(), || {
        format!(
            "a person's presence moved: {:?} {:?}",
            e.view_address, e.time
        )
    })?;
    // the host's setter, on the agent's connection
    f.s.set_view_address(bot, Some("@tr:v1")).unwrap();
    let e = f.entry(ada, "atlas");
    need(e.view_address.as_deref() == Some("@tr:v1"), || {
        format!("the host's setter on the agent: {:?}", e.view_address)
    })
}

#[test]
fn an_agent_verb_sets_its_view_address_and_time_position() {
    row_an_agent_verb_sets_them().unwrap();
}

/// Nobody's view address or time position is there until set: not on a fresh row or device,
/// and no roster event names one while others move their pointer and selection.
#[test]
fn absent_stays_absent() {
    let f = fx("absent");
    let ada = f.join("ada", ParticipantKind::Person);
    let bob = f.join("bob", ParticipantKind::Person);
    let rx = f.s.subscribe_roster(ada).unwrap();
    f.tick();
    f.s.set_pointer(
        bob,
        Some(k2_mpc_base::session::Point::new(1.0, 2.0).unwrap()),
    )
    .unwrap();
    f.tick();
    for pid in ["ada", "bob"] {
        let e = f.entry(ada, pid);
        assert!(e.view_address.is_none() && e.time.is_none(), "{pid}: {e:?}");
        for d in &e.devices {
            assert!(d.view_address.is_none() && d.time.is_none(), "{pid}: {d:?}");
        }
    }
    let evs = drain(&rx);
    assert!(
        evs.iter().any(|e| matches!(e, RosterEvent::Pointer { .. })),
        "control: the pointer moved: {evs:?}"
    );
    assert!(
        !evs.iter().any(|e| matches!(
            e,
            RosterEvent::ViewAddress { .. } | RosterEvent::TimePosition { .. }
        )),
        "an event named a value nobody set: {evs:?}"
    );
}

/// Another viewer gets Ada's view address and playhead at the next roster flush, and their
/// clearing; an App viewer gets them with Ada as a handle.
fn row_a_viewer_sees_them() -> Result<(), String> {
    let f = fx("viewer");
    let ada = f.join("ada", ParticipantKind::Person);
    let bob = f.join("bob", ParticipantKind::Person);
    let app = f.join("guest", ParticipantKind::App);
    let rb = f.s.subscribe_roster(bob).unwrap();
    let ra = f.s.subscribe_roster(app).unwrap();
    f.tick();
    drain(&rb);
    drain(&ra);
    f.s.set_view_address(ada, Some("@clip:c1")).unwrap();
    let t = f.s.set_time_position(ada, Some(playing("42"))).unwrap();
    f.tick();
    let evs = drain(&rb);
    need(
        evs.iter().any(|e| {
            matches!(e,
            RosterEvent::ViewAddress { who, address: Some(a), .. }
                if who.to_string() == "ada" && a == "@clip:c1")
        }),
        || format!("Bob got no view address of Ada's: {evs:?}"),
    )?;
    need(
        evs.iter().any(|e| {
            matches!(e,
            RosterEvent::TimePosition { who, time: Some(x), .. }
                if who.to_string() == "ada" && Some(x) == t.as_ref())
        }),
        || format!("Bob got no time position of Ada's: {evs:?}"),
    )?;
    let e = f.entry(bob, "ada");
    need(
        e.view_address.as_deref() == Some("@clip:c1") && e.time == t,
        || format!("Bob's roster row of Ada: {:?} {:?}", e.view_address, e.time),
    )?;
    let app_evs = drain(&ra);
    need(
        app_evs.iter().any(|e| {
            matches!(e,
            RosterEvent::ViewAddress { who, address: Some(_), .. }
                if who.to_string().starts_with('p'))
        }),
        || format!("the App viewer's view address event, by handle: {app_evs:?}"),
    )?;
    f.s.set_time_position(ada, None).unwrap();
    f.tick();
    let evs = drain(&rb);
    need(
        evs.iter().any(|e| {
            matches!(e,
            RosterEvent::TimePosition { who, time: None, .. } if who.to_string() == "ada")
        }),
        || format!("Bob didn't hear Ada's playhead go: {evs:?}"),
    )
}

#[test]
fn a_viewer_sees_anothers_view_address_and_playhead() {
    row_a_viewer_sees_them().unwrap();
}

/// What isn't a time position or an address is refused, and nothing changes; a watcher is
/// refused both.
#[test]
fn bad_values_are_refused() {
    for at in ["", "01", "-0", "1.5", "1e3", "+1", "99999999999999999999"] {
        assert!(
            matches!(
                TimePosition::new(at, PlayState::Paused),
                Err(SessionError::BadPosition(_))
            ),
            "at {at:?}"
        );
    }
    let t = TimePosition::new("-12", PlayState::Paused).unwrap();
    for r in ["", "0/0", "1/0", "1/-2", "x", "1.5", "2/"] {
        assert!(
            matches!(t.clone().rate(r), Err(SessionError::BadPosition(_))),
            "rate {r:?}"
        );
    }
    for r in ["1", "-1", "0", "1001/1000", "1/2"] {
        assert!(t.clone().rate(r).is_ok(), "rate {r:?}");
    }
    let long = format!("@tr:{}", "v".repeat(PRESENCE_ADDRESS_MAX));
    for a in ["", "@tr", "@Tr:1", "@k:BAD", long.as_str()] {
        assert!(
            matches!(t.clone().within(a), Err(SessionError::BadPosition(_))),
            "in {a:?}"
        );
    }
    let f = fx("refused");
    let ada = f.join("ada", ParticipantKind::Person);
    f.s.set_view_address(ada, Some("@tr:v1")).unwrap();
    for a in ["", "@tr", long.as_str()] {
        assert!(
            matches!(
                f.s.set_view_address(ada, Some(a)),
                Err(SessionError::BadPosition(_))
            ),
            "view address {a:?}"
        );
    }
    assert_eq!(
        f.entry(ada, "ada").view_address.as_deref(),
        Some("@tr:v1"),
        "a refusal changes nothing"
    );
}

/// The view schema's time unit is on the wire only when declared, and checked.
#[test]
fn the_time_unit_is_declared_and_checked() {
    assert_eq!(
        VIEW.to_value()
            .get("time_unit")
            .and_then(|v| match v {
                k2_mpc_base::errors::Value::Str(s) => Some(s.clone()),
                _ => None,
            })
            .as_deref(),
        Some("tick")
    );
    let none = ViewSchema {
        time_unit: None,
        ..VIEW
    };
    assert!(none.to_value().get("time_unit").is_none());
    VIEW.check(&SCHEMA, Toy.composite()).unwrap();
    for bad in ["", "two words", "ticks!"] {
        let b: &'static str = Box::leak(bad.to_owned().into_boxed_str());
        let vs = ViewSchema {
            time_unit: Some(b),
            ..VIEW
        };
        let e = vs.check(&SCHEMA, Toy.composite()).expect_err(bad);
        assert!(e.contains("time unit"), "{bad:?}: {e}");
    }
}
