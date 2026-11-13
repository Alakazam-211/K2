//! The kit side of the presence (r1.4, Items 3 and 4, 27 and
//! 30) on a deck-shaped keyed doc: `slides` and `shapes` (MovableLists) and `fields` (a Map of
//! `<shape>\x1f<field>`, composite on `shapes`), whose pack names a shape's slide as its
//! container (`s259-3` is on `s259`). Public API only.
//!
//! Planted faults caught by name (`k2_mpc_base::differential::ContractMutant`): each
//! `plant_*_is_caught` row runs its check under the fault and requires it to fail where it says.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use k2_mpc_base::differential::{with_contract_mutant, ContractMutant};
use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{ListOp, MapOp};
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Declined, EditOptions, ElemFocus, ElemRange, Host, Joined, Pack,
    PackContent, PackError, Participant, ParticipantKind, Rights, RosterEntry, RosterEvent,
    Session, SessionError, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::{Edit, Key, Value};

const SLIDES: &str = "slides";
const SHAPES: &str = "shapes";
const FIELDS: &str = "fields";

/// A deck-shaped keyed doc. The file is one line a slide (`s256`) followed by its shapes
/// (`  s256-2`); a shape's `text` field starts empty.
struct Deck;

const DECK: Schema = Schema {
    name: "deck-lite",
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
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let (mut slides, mut shapes, mut fields) = (Vec::new(), Vec::new(), BTreeMap::new());
        for line in std::str::from_utf8(b).unwrap().lines() {
            let k = Key::new(line.trim()).unwrap();
            if line.starts_with(' ') {
                fields.insert(format!("{k}\u{1f}text"), Value::Str(String::new()));
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

struct TmpDir(std::path::PathBuf);
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

fn fx_with(tag: &str, set: impl FnOnce(&mut Config)) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-contract-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(100_000),
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    set(&mut c);
    let mgr = SessionManager::new(store, host.clone(), c);
    let s = mgr
        .open(
            Arc::new(Deck),
            &DocKey {
                space: "ws".into(),
                path: "sample.deck".into(),
            },
            Some(SAMPLE),
        )
        .unwrap();
    Fx {
        _dir: TmpDir(dir),
        host,
        mgr,
        s,
    }
}

fn fx(tag: &str) -> Fx {
    fx_with(tag, |_| {})
}

impl Fx {
    fn tick(&self) {
        self.host.now.fetch_add(300, Ordering::SeqCst);
        self.mgr.flush_rosters();
    }
    fn person(&self, pid: &str) -> Joined {
        self.s
            .join(
                Participant::new(pid, pid, "#2f9e8f", ParticipantKind::Person, "tab"),
                Stay::Held,
            )
            .unwrap()
    }
    fn agent(&self, pid: &str) -> Joined {
        self.s
            .join(
                Participant::new(pid, "Atlas", "#6d5bd0", ParticipantKind::Agent, "cli"),
                Stay::Held,
            )
            .unwrap()
    }
    fn reader(&self, pid: &str) {
        self.host
            .rights
            .lock()
            .unwrap()
            .insert(pid.into(), Rights::read_only());
    }
    fn entry(&self, c: ConnId, pid: &str) -> RosterEntry {
        self.s
            .roster(c)
            .unwrap()
            .into_iter()
            .find(|e| e.participant.pid == pid)
            .unwrap_or_else(|| panic!("{pid} isn't in the roster"))
    }
    /// A person's write of `shape`'s `text`.
    fn set_text(&self, c: ConnId, shape: &str, text: &str) {
        self.set_field(c, shape, "text", text);
    }
    fn set_field(&self, c: ConnId, shape: &str, field: &str, text: &str) {
        self.s
            .edit_ops(
                c,
                vec![Edit::Map {
                    root: FIELDS.into(),
                    ops: vec![MapOp::Set {
                        key: format!("{shape}\u{1f}{field}"),
                        value: Value::Str(text.into()),
                    }],
                }],
                &EditOptions::new(),
            )
            .unwrap();
    }
    fn delete(&self, c: ConnId, root: &str, key: &str) {
        self.s
            .edit_ops(
                c,
                vec![Edit::List {
                    root: root.into(),
                    ops: vec![ListOp::Delete { key: k(key) }],
                }],
                &EditOptions::new(),
            )
            .unwrap();
    }
}

fn k(s: &str) -> Key {
    Key::new(s).unwrap()
}

fn focus(root: &str, key: &str) -> Option<ElemFocus> {
    Some(ElemFocus::new(root, k(key)))
}

fn range(key: &str, anchor: usize, head: usize) -> Option<ElemRange> {
    Some(ElemRange::new(SHAPES, k(key), "text", anchor, head))
}

fn id_of(e: SessionError) -> String {
    e.report().id().to_owned()
}

fn drain(rx: &Receiver<RosterEvent>) -> Vec<RosterEvent> {
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

fn focus_key(e: &RosterEntry) -> Option<String> {
    e.focus.as_ref().map(|f| format!("{}:{}", f.root, f.key))
}

// ---------- focus ----------

/// Focus needs read (a reader may set it), names a live element of a list root, and leaves when
/// that element goes. Refusals leave it as it was.
#[test]
fn focus_needs_read_names_a_live_element_and_leaves_with_it() {
    let f = fx("focus");
    f.reader("kai");
    let kai = f.person("kai");
    let ada = f.person("ada");
    f.s.set_focus(kai.conn, focus(SLIDES, "s257")).unwrap();
    assert_eq!(
        focus_key(&f.entry(ada.conn, "kai")).as_deref(),
        Some("slides:s257")
    );
    for (bad, why) in [
        (focus(SLIDES, "s999"), "an unknown key"),
        (focus(FIELDS, "s256"), "a root that isn't a list"),
    ] {
        let e = f.s.set_focus(kai.conn, bad).unwrap_err();
        assert_eq!(id_of(e), "MPC-E114", "{why}");
    }
    assert_eq!(
        focus_key(&f.entry(ada.conn, "kai")).as_deref(),
        Some("slides:s257"),
        "a refusal leaves the focus as it was"
    );
    f.delete(ada.conn, SLIDES, "s257");
    assert_eq!(focus_key(&f.entry(ada.conn, "kai")), None, "the slide went");
    f.s.set_focus(ada.conn, focus(SLIDES, "s256")).unwrap();
    f.s.set_focus(ada.conn, None).unwrap();
    assert_eq!(focus_key(&f.entry(ada.conn, "ada")), None, "cleared");
}

/// A watcher (past `participants_max`) is refused focus and range (`MPC-E305`) as every
/// element setter, and has no roster row; its count is the roster's `watching`.
#[test]
fn a_watcher_is_refused_focus_and_range() {
    let f = fx_with("watcher", |c| c.participants_max = 1);
    let ada = f.person("ada");
    let kai = f.person("kai");
    let e = f.s.set_focus(kai.conn, focus(SLIDES, "s260")).unwrap_err();
    assert_eq!(id_of(e), "MPC-E305");
    let e = f.s.set_focus(kai.conn, focus(SLIDES, "s256")).unwrap_err();
    assert_eq!(
        id_of(e),
        "MPC-E305",
        "a live key too: the watcher, not the key"
    );
    let e = f.s.set_range(kai.conn, range("s256-2", 0, 0)).unwrap_err();
    assert_eq!(id_of(e), "MPC-E305");
    assert!(f
        .s
        .roster(ada.conn)
        .unwrap()
        .iter()
        .all(|e| e.participant.pid != "kai"));
}

// ---------- range ----------

/// The check behind the range rows: Ada focuses slide 1 (s256), then sets a range in slide 4's
/// body (s259-3): her focus is slide 4 (contract case A). `Err` says what was wrong.
fn range_sets_focus() -> Result<(), String> {
    let f = fx("range-focus");
    let ada = f.person("ada");
    let bob = f.person("bob");
    f.set_text(bob.conn, "s259-3", "Hire two engineers\nShip the v2 beta");
    f.s.set_focus(ada.conn, focus(SLIDES, "s256")).unwrap();
    f.s.set_range(ada.conn, range("s259-3", 0, 18)).unwrap();
    let e = f.entry(bob.conn, "ada");
    if focus_key(&e).as_deref() != Some("slides:s259") {
        return Err(format!(
            "after a range on s259-3, focus {:?}, want slides:s259",
            focus_key(&e)
        ));
    }
    let r = e.range.ok_or("no range")?;
    if (r.key.as_str(), r.field.as_str(), r.anchor, r.head) != ("s259-3", "text", 0, 18) {
        return Err(format!("range {r:?}"));
    }
    Ok(())
}

#[test]
fn a_range_sets_the_focus_to_its_container() {
    range_sets_focus().unwrap();
}

/// The planted fault `RangeDoesNotSetFocus`: the check fails at the focus.
#[test]
fn plant_range_does_not_set_focus_is_caught() {
    let e = with_contract_mutant(ContractMutant::RangeDoesNotSetFocus, range_sets_focus)
        .expect_err("the check passes with the focus left on slide 1");
    assert!(e.starts_with("after a range on s259-3, focus"), "{e}");
}

/// A range is dropped, never moved, when its field changes (anyone's change, its owner's too);
/// a change to another field, or to another shape's, leaves it; its element's deletion drops
/// it. The focus stays through a dropped range (contract case A).
#[test]
fn a_range_is_dropped_when_its_field_changes_never_moved() {
    let f = fx("range-drop");
    let ada = f.person("ada");
    let atlas = f.agent("atlas");
    f.set_text(ada.conn, "s259-3", "Hire two engineers\nShip the v2 beta");
    f.set_text(ada.conn, "s259-2", "Next steps");
    f.s.set_range(ada.conn, range("s259-3", 0, 18)).unwrap();
    // another shape's text, another field of the same shape: kept
    f.set_text(ada.conn, "s259-2", "Next steps!");
    f.set_field(ada.conn, "s259-3", "name", "Body 3");
    assert!(f.entry(ada.conn, "ada").range.is_some(), "kept");
    // its field changes (an agent's verb): dropped, not moved to "Hire three engineers"
    let base = f.s.status().unwrap().version;
    f.s.edit_ops(
        atlas.conn,
        vec![Edit::Map {
            root: FIELDS.into(),
            ops: vec![MapOp::Set {
                key: "s259-3\u{1f}text".into(),
                value: Value::Str("Hire three engineers\nShip the v2 beta".into()),
            }],
        }],
        &EditOptions::new().base(base),
    )
    .unwrap();
    let e = f.entry(ada.conn, "ada");
    assert!(e.range.is_none(), "dropped: {:?}", e.range);
    assert_eq!(
        focus_key(&e).as_deref(),
        Some("slides:s259"),
        "the focus stays"
    );
    // the owner's own change drops it too
    f.s.set_range(ada.conn, range("s259-3", 0, 4)).unwrap();
    f.set_text(ada.conn, "s259-3", "Hire");
    assert!(f.entry(ada.conn, "ada").range.is_none(), "her own change");
    // its element goes
    f.s.set_range(ada.conn, range("s259-3", 0, 4)).unwrap();
    f.delete(ada.conn, SHAPES, "s259-3");
    assert!(f.entry(ada.conn, "ada").range.is_none(), "the shape went");
}

/// What a range is refused for (`MPC-E114`, the range as it was), and that it needs edit as a
/// text caret does (a reader: `MPC-E102`). Offsets are UTF-8 bytes on character boundaries.
#[test]
fn a_range_is_checked_against_the_fields_text() {
    let f = fx("range-bad");
    f.reader("kai");
    let ada = f.person("ada");
    let kai = f.person("kai");
    f.set_text(ada.conn, "s259-3", "Café plan");
    f.s.set_range(ada.conn, range("s259-3", 0, 5)).unwrap();
    let cases = [
        (range("s259-3", 0, 4), "inside é (UTF-8 bytes, not UTF-16)"),
        (range("s259-3", 0, 11), "past the end"),
        (range("s999-1", 0, 0), "an unknown shape"),
        (
            Some(ElemRange::new(SHAPES, k("s259-3"), "nope", 0, 0)),
            "no such field",
        ),
        (
            Some(ElemRange::new(SLIDES, k("s259"), "text", 0, 0)),
            "a list with no fields",
        ),
    ];
    for (bad, why) in cases {
        let e = f.s.set_range(ada.conn, bad).unwrap_err();
        assert_eq!(id_of(e), "MPC-E114", "{why}");
    }
    let r = f.entry(ada.conn, "ada").range.unwrap();
    assert_eq!(
        (r.anchor, r.head),
        (0, 5),
        "a refusal leaves the range as it was"
    );
    let e = f.s.set_range(kai.conn, range("s259-3", 0, 0)).unwrap_err();
    assert_eq!(id_of(e), "MPC-E102", "a reader has no caret");
    f.s.set_range(ada.conn, None).unwrap();
    assert!(f.entry(ada.conn, "ada").range.is_none());
}

/// Focus and range reach subscribers as roster events (wire 3), one a connection a flush, the
/// focus before the range; a dropped range is `None`.
#[test]
fn focus_and_range_reach_the_roster_push() {
    let f = fx("push");
    let ada = f.person("ada");
    let bob = f.person("bob");
    f.set_text(bob.conn, "s259-3", "Hire two engineers");
    let rx = f.s.subscribe_roster(bob.conn).unwrap();
    f.tick();
    drain(&rx);
    f.s.set_range(ada.conn, range("s259-3", 0, 18)).unwrap();
    f.tick();
    f.set_text(bob.conn, "s259-3", "Hire three engineers");
    f.tick();
    let got: Vec<String> = drain(&rx)
        .into_iter()
        .filter_map(|e| match e {
            RosterEvent::Focus { who, focus, .. } if who.pid() == Some("ada") => Some(format!(
                "focus {}",
                focus.map_or("-".into(), |f| f.key.to_string())
            )),
            RosterEvent::Range { who, range, .. } if who.pid() == Some("ada") => Some(format!(
                "range {}",
                range.map_or("-".into(), |r| format!(
                    "{}:{}..{}",
                    r.key, r.anchor, r.head
                ))
            )),
            _ => None,
        })
        .collect();
    assert_eq!(got, ["focus s259", "range s259-3:0..18", "range -"]);
}

// ---------- claim events, and a claim's start on an agent's presence ----------

fn keys(v: &[&str]) -> Vec<Key> {
    v.iter().map(|x| k(x)).collect()
}

fn selected(e: &RosterEntry) -> Vec<String> {
    e.selection.as_ref().map_or(Vec::new(), |s| {
        s.keys.iter().map(|k| format!("{}:{k}", s.root)).collect()
    })
}

/// The check behind the claim rows (contract GOLD `events-timeline.json`): Atlas has shape
/// s257-7 selected; its claim on slides 259 and 258 (positions 4 and 3) sets its focus to 258,
/// the first in document order, and replaces its selection with the claimed slides, in the
/// order given (c-14); its claim on shape s256-2 sets its focus to that shape's slide and its
/// selection to the shape (c-11). `Err` says what was wrong.
fn claim_sets_focus_and_replaces_selection() -> Result<(), String> {
    let f = fx("claim-presence");
    let ada = f.person("ada");
    let atlas = f.agent("atlas");
    f.s.set_selection(
        atlas.conn,
        Some(k2_mpc_base::session::ElemSelection::new(
            SHAPES,
            keys(&["s257-7"]),
        )),
    )
    .unwrap();
    f.s.claim_elems(
        atlas.conn,
        SLIDES,
        &keys(&["s259", "s258"]),
        None,
        Some("putting next steps before the numbers".into()),
        false,
    )
    .unwrap();
    let e = f.entry(ada.conn, "atlas");
    if selected(&e) != ["slides:s259", "slides:s258"] {
        return Err(format!(
            "after claiming slides 259 and 258, selection {:?}, want the claimed slides",
            selected(&e)
        ));
    }
    if focus_key(&e).as_deref() != Some("slides:s258") {
        return Err(format!("focus {:?}, want slides:s258", focus_key(&e)));
    }
    f.s.claim_elems(atlas.conn, SHAPES, &keys(&["s256-2"]), None, None, true)
        .unwrap();
    let e = f.entry(ada.conn, "atlas");
    if selected(&e) != ["shapes:s256-2"] || focus_key(&e).as_deref() != Some("slides:s256") {
        return Err(format!(
            "after claiming s256-2: selection {:?}, focus {:?}",
            selected(&e),
            focus_key(&e)
        ));
    }
    Ok(())
}

#[test]
fn an_agents_claim_sets_its_focus_and_replaces_its_selection() {
    claim_sets_focus_and_replaces_selection().unwrap();
}

/// The planted fault `ClaimDoesNotReplaceSelection`: the check fails at the selection.
#[test]
fn plant_claim_does_not_replace_selection_is_caught() {
    let e = with_contract_mutant(
        ContractMutant::ClaimDoesNotReplaceSelection,
        claim_sets_focus_and_replaces_selection,
    )
    .expect_err("the check passes with the old selection kept");
    assert!(
        e.starts_with("after claiming slides 259 and 258, selection"),
        "{e}"
    );
}

/// A person's claim leaves their focus and selection to them.
#[test]
fn a_persons_claim_leaves_their_focus_and_selection() {
    let f = fx("claim-person");
    let ada = f.person("ada");
    f.s.set_focus(ada.conn, focus(SLIDES, "s257")).unwrap();
    f.s.claim_elems(ada.conn, SHAPES, &keys(&["s259-2"]), None, None, false)
        .unwrap();
    let e = f.entry(ada.conn, "ada");
    assert_eq!(focus_key(&e).as_deref(), Some("slides:s257"));
    assert!(e.selection.is_none());
}

/// `claim_started` and `claim_ended` on the presence stream carry the actor (id as `who`,
/// name, kind, colour) and times: `started_ms`; `at_ms`, and `at_rev`, the change that ended
/// it (`edited`, `deleted`) or `None` (`released`). In a flush, the claim's start comes before
/// the focus and selection it set.
#[test]
fn claim_events_carry_the_actor_and_their_times() {
    let f = fx("claim-events");
    let ada = f.person("ada");
    let atlas = f.agent("atlas");
    let rx = f.s.subscribe_roster(ada.conn).unwrap();
    f.tick();
    drain(&rx);
    let t0 = f.host.now.load(Ordering::SeqCst);
    // c-11: not held, ends on Atlas's own edit
    f.s.claim_elems(
        atlas.conn,
        SHAPES,
        &keys(&["s256-2"]),
        None,
        Some("rewriting the title".into()),
        false,
    )
    .unwrap();
    f.tick();
    let base = f.s.status().unwrap().version;
    f.s.edit_ops(
        atlas.conn,
        vec![Edit::Map {
            root: FIELDS.into(),
            ops: vec![MapOp::Set {
                key: "s256-2\u{1f}text".into(),
                value: Value::Str("Q3 review".into()),
            }],
        }],
        &EditOptions::new().base(base),
    )
    .unwrap();
    let rev = f.s.status().unwrap().version;
    let t1 = f.host.now.load(Ordering::SeqCst);
    f.tick();
    // c-13: held, released
    let c13 =
        f.s.claim_elems(atlas.conn, SLIDES, &keys(&["s257"]), None, None, true)
            .unwrap();
    f.tick();
    f.s.release_claim(atlas.conn, Some(c13.id)).unwrap();
    f.tick();
    let evs = drain(&rx);
    let mut seen = Vec::new();
    for e in &evs {
        match e {
            RosterEvent::ClaimStartedElems {
                who,
                actor,
                started_ms,
                ..
            } => {
                assert_eq!(who.pid(), Some("atlas"));
                assert_eq!(
                    (actor.name.as_str(), actor.kind, actor.colour.as_str()),
                    ("Atlas", ParticipantKind::Agent, "#6d5bd0")
                );
                seen.push(format!("started@{}", started_ms - t0));
            }
            RosterEvent::ClaimEnded {
                how,
                actor,
                at_ms,
                at_rev,
                ..
            } => {
                assert_eq!(actor.name, "Atlas");
                seen.push(format!(
                    "ended {} at_rev {:?} at_ms {}",
                    how.as_str(),
                    at_rev,
                    if *at_ms == t1 { "the edit's" } else { "later" }
                ));
            }
            RosterEvent::Focus { who, .. } if who.pid() == Some("atlas") => {
                seen.push("focus".into())
            }
            RosterEvent::Selection { who, .. } if who.pid() == Some("atlas") => {
                seen.push("selection".into())
            }
            _ => {}
        }
    }
    let first = seen.iter().position(|s| s.starts_with("started")).unwrap();
    assert_eq!(seen[first..first + 3], ["started@0", "focus", "selection"]);
    assert!(
        seen.contains(&format!("ended edited at_rev Some({rev}) at_ms the edit's")),
        "{seen:?}"
    );
    assert!(
        seen.iter()
            .any(|s| s.starts_with("ended released at_rev None")),
        "{seen:?}"
    );
}

// ---------- the watcher count ----------

/// The presence stream's `watching {count}` (contract GOLD case H): Kai joins past the cap, so
/// the count goes to 1 with no row, no `Joined` and no presence for him; Kai leaves, so it goes
/// back to 0, with no `Left`. A snapshot carries the count too.
#[test]
fn the_watcher_count_follows_watchers_and_they_get_no_row() {
    let f = fx_with("watching", |c| c.participants_max = 2);
    let ada = f.person("ada");
    let _atlas = f.agent("atlas");
    let rx = f.s.subscribe_roster(ada.conn).unwrap();
    f.tick();
    drain(&rx);
    let kai = f.person("kai");
    f.tick();
    let joined = drain(&rx);
    let late = drain(&f.s.subscribe_roster(ada.conn).unwrap());
    f.s.leave(kai.conn).unwrap();
    f.tick();
    let left = drain(&rx);
    let about_kai = |evs: &[RosterEvent]| {
        evs.iter()
            .filter(|e| match e {
                RosterEvent::Joined { who, .. } | RosterEvent::Left { who, .. } => {
                    who.pid() == Some("kai")
                }
                _ => false,
            })
            .count()
    };
    let counts = |evs: &[RosterEvent]| -> Vec<usize> {
        evs.iter()
            .filter_map(|e| match e {
                RosterEvent::Watching { count } => Some(*count),
                _ => None,
            })
            .collect()
    };
    assert_eq!(counts(&joined), [1], "{joined:?}");
    assert_eq!(counts(&left), [0], "{left:?}");
    assert_eq!(
        about_kai(&joined) + about_kai(&left),
        0,
        "no Joined or Left for a watcher"
    );
    match &late[0] {
        RosterEvent::Snapshot {
            watching, members, ..
        } => {
            assert_eq!(*watching, 1);
            assert!(members.iter().all(|m| m.entry.participant.pid != "kai"));
        }
        other => panic!("not a snapshot: {other:?}"),
    }
}
