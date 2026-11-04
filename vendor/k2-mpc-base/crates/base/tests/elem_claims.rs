//! Claims on elements of a list root (`Session::claim_elems`), on a synthetic keyed doc that
//! isn't a table: a MovableList `items` with Maps `props` (`<item>\x1f<field>`) and `links`
//! (`<item>\x1f{from,to}`) declared composite on it, a second list `groups`, an undeclared Map
//! `marks` and a text root `notes`.
//!
//! `MPC_STRUCT_MUTANT` plants a fault for the whole suite (`k2_mpc_base::differential`, the
//! `differential` feature, on for these tests):
//! - `map-keys-unmapped` is `ClaimMutant::MapKeysUnmapped`, caught by
//!   `a_write_to_a_claimed_elements_map_key_is_refused`;
//! - `mark-before-apply` is `ClaimMutant::MarkBeforeApply`, caught by
//!   `a_change_that_does_not_land_leaves_the_claim_alone`,
//!   `a_refused_change_leaves_the_claim_alone`,
//!   `an_op_with_no_effect_beside_one_that_lands_leaves_the_claim_alone` and
//!   `a_change_that_does_not_land_leaves_the_reservation_alone`;
//! - `recover-keeps-claims` is `MarkMutant::RecoverKeepsClaims`, caught by
//!   `a_change_whose_bookkeeping_fails_resets_the_claim`.
//!
//! Unset, the tests run as is. Any other value stops the suite.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::doc::{DocError, Neutral};
use k2_mpc_base::elem::{Anchor, KeyedRoot, ListOp, MapOp, OpRefusal};
use k2_mpc_base::session::{
    elem_of_map_key, AuditEvent, ClaimEnd, ClaimId, ClaimPolicy, ClaimSpan, Config, ConnId,
    Declined, EditOptions, Host, Pack, PackContent, PackError, Participant, ParticipantKind,
    ReservationEnd, Rights, RosterEvent, Session, SessionError, SessionManager, Stay, Who,
    CLAIM_KEYS_MAX, CLAIM_NOTE_MAX,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::{Edit, Key, Kind, Schema, Value};

fn under_struct_mutant<R>(f: impl FnOnce() -> R) -> R {
    use k2_mpc_base::differential::{with_claim_mutant, ClaimMutant};
    let m = std::env::var("MPC_STRUCT_MUTANT");
    println!("MPC_STRUCT_MUTANT={}", m.as_deref().unwrap_or("(unset)"));
    match m.as_deref() {
        Ok("map-keys-unmapped") => with_claim_mutant(ClaimMutant::MapKeysUnmapped, f),
        Ok("mark-before-apply") => with_claim_mutant(ClaimMutant::MarkBeforeApply, f),
        Ok("recover-keeps-claims") => {
            use k2_mpc_base::differential::{with_mark_mutant, MarkMutant};
            with_mark_mutant(MarkMutant::RecoverKeepsClaims, f)
        }
        Ok(other) => panic!("MPC_STRUCT_MUTANT={other}: not a mutant this suite plants"),
        Err(_) => f(),
    }
}

/// One element a line (`items i1`, `groups g1`); the rest of the roots start empty.
struct ItemsPack;

impl Pack for ItemsPack {
    fn schema(&self) -> Schema {
        Schema {
            name: "items",
            roots: &[
                ("items", Kind::MovableList),
                ("groups", Kind::MovableList),
                ("props", Kind::Map),
                ("links", Kind::Map),
                ("marks", Kind::Map),
                ("notes", Kind::Text),
            ],
        }
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        &[("props", "items"), ("links", "items")]
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let (mut items, mut groups) = (Vec::new(), Vec::new());
        for l in std::str::from_utf8(b).unwrap().lines() {
            let (root, k) = l.split_once(' ').unwrap();
            let e = (Key::new(k).unwrap(), Value::Null);
            if root == "items" {
                items.push(e);
            } else {
                groups.push(e);
            }
        }
        Ok(PackContent {
            roots: vec![
                ("items".into(), Neutral::List(items)),
                ("groups".into(), Neutral::List(groups)),
                ("props".into(), Neutral::Map(Default::default())),
                ("links".into(), Neutral::Map(Default::default())),
                ("marks".into(), Neutral::Map(Default::default())),
                ("notes".into(), Neutral::Text("first\nsecond\n".into())),
            ],
            codec: Vec::new(),
        })
    }
    fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        let mut out = String::new();
        for (root, n) in c {
            if let Neutral::List(es) = n {
                for (k, _) in es {
                    out.push_str(&format!("{root} {k}\n"));
                }
            }
        }
        Ok(out.into_bytes())
    }
}

/// A table's six roots, for the one refusal that needs a table.
struct TablePack;

const TABLE: Schema = Schema {
    name: "table-lite",
    roots: k2_mpc_base::table::ROOTS,
};

impl Pack for TablePack {
    fn schema(&self) -> Schema {
        TABLE
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        use k2_mpc_base::table::{Field, Shape, Table};
        let recs: Vec<Vec<Field>> = std::str::from_utf8(b)
            .unwrap()
            .lines()
            .map(|l| l.split(',').map(Field::new).collect())
            .collect();
        let (_, edits) = Table::import(&recs, false, Shape::detect(&recs)).unwrap();
        let mut d = k2_mpc_base::Doc::new(TABLE).unwrap();
        d.edit_as(2, &edits).unwrap();
        Ok(PackContent {
            roots: d.export_neutral().unwrap(),
            codec: vec![1],
        })
    }
    fn encode(&self, _: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        Ok(Vec::new())
    }
}

/// A clock that moves only when a test moves it; every event kept; read-only for `viewer`.
struct H {
    now: AtomicU64,
    events: Mutex<Vec<AuditEvent>>,
}

impl Host for H {
    fn rights(&self, p: &Participant, _: &DocKey) -> Rights {
        if p.pid == "viewer" {
            Rights::read_only()
        } else {
            Rights::full()
        }
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, e: &AuditEvent) {
        self.events.lock().unwrap().push(e.clone());
    }
}

struct Fx {
    mgr: SessionManager,
    host: Arc<H>,
    store: Arc<DocStore>,
    s: Arc<Session>,
    dir: std::path::PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const FILE: &[u8] = b"items i1\nitems i2\nitems i3\nitems i4\ngroups i1\ngroups g2\n";

fn fx_with(name: &str, config: Config) -> Fx {
    let dir = std::env::temp_dir().join(format!("k2mpc-elem-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("d.redb")).unwrap());
    let host = Arc::new(H {
        now: AtomicU64::new(1_000_000),
        events: Mutex::new(Vec::new()),
    });
    let mgr = SessionManager::new(store.clone(), host.clone(), config);
    let key = DocKey {
        space: "ws".into(),
        path: "a.items".into(),
    };
    let s = mgr.open(Arc::new(ItemsPack), &key, Some(FILE)).unwrap();
    Fx {
        mgr,
        host,
        store,
        s,
        dir,
    }
}

fn fx(name: &str) -> Fx {
    fx_with(name, Config::default())
}

impl Fx {
    fn join(&self, pid: &str, kind: ParticipantKind) -> ConnId {
        let p = Participant::new(pid, pid, "#000000", kind, "app");
        self.s.join(p, Stay::Held).unwrap().conn
    }
    fn person(&self, pid: &str) -> ConnId {
        self.join(pid, ParticipantKind::Person)
    }
    fn agent(&self, pid: &str) -> ConnId {
        self.join(pid, ParticipantKind::Agent)
    }
    fn version(&self) -> u64 {
        self.s.status().unwrap().version
    }
    /// An edit at the current version under `policy`.
    fn edit(&self, c: ConnId, policy: ClaimPolicy, edits: Vec<Edit>) -> Result<(), SessionError> {
        let o = EditOptions::new().policy(policy).base(self.version());
        self.s.edit_ops(c, edits, &o).map(|_| ())
    }
    fn respect(&self, c: ConnId, edits: Vec<Edit>) -> Result<(), SessionError> {
        self.edit(c, ClaimPolicy::Respect, edits)
    }
    fn ended(&self, id: ClaimId) -> Option<ClaimEnd> {
        self.host
            .events
            .lock()
            .unwrap()
            .iter()
            .find_map(|e| match e {
                AuditEvent::ClaimEnded { claim, how, .. } if *claim == id => Some(*how),
                _ => None,
            })
    }
    fn map_value(&self, root: &str, key: &str) -> Option<Value> {
        let st = self.s.keyed_state_for_tests().unwrap();
        match st.roots.iter().find(|(r, _)| r == root) {
            Some((_, KeyedRoot::Map(m))) => m.get(key).cloned(),
            other => panic!("{root}: {other:?}"),
        }
    }
    fn live(&self, root: &str) -> Vec<String> {
        let st = self.s.keyed_state_for_tests().unwrap();
        match st.roots.iter().find(|(r, _)| r == root) {
            Some((_, KeyedRoot::List { live, .. })) => {
                live.iter().map(|(k, _)| k.to_string()).collect()
            }
            other => panic!("{root}: {other:?}"),
        }
    }
    fn tick(&self) {
        self.host.now.fetch_add(300, Ordering::SeqCst);
        self.mgr.flush_rosters();
    }
}

fn k(s: &str) -> Key {
    Key::new(s).unwrap()
}

fn ks(v: &[&str]) -> Vec<Key> {
    v.iter().map(|s| k(s)).collect()
}

fn set_prop(item: &str, field: &str) -> Vec<Edit> {
    set_in("props", item, field)
}

fn set_in(root: &str, item: &str, field: &str) -> Vec<Edit> {
    vec![Edit::Map {
        root: root.into(),
        ops: vec![MapOp::Set {
            key: format!("{item}\u{1f}{field}"),
            value: Value::str("v"),
        }],
    }]
}

fn list(op: ListOp) -> Vec<Edit> {
    vec![Edit::List {
        root: "items".into(),
        ops: vec![op],
    }]
}

fn delete(key: &str) -> Vec<Edit> {
    list(ListOp::Delete { key: k(key) })
}

fn id_of(e: &SessionError) -> &'static str {
    e.report().id()
}

#[test]
fn elem_of_map_key_reads_declared_roots_only() {
    let c: &[(&str, &str)] = &[("props", "items"), ("links", "items")];
    assert_eq!(
        elem_of_map_key(c, "props", "i1\u{1f}x"),
        Some(("items", k("i1")))
    );
    assert_eq!(
        elem_of_map_key(c, "links", "i1\u{1f}from"),
        Some(("items", k("i1")))
    );
    // the element is what comes before the first separator
    assert_eq!(
        elem_of_map_key(c, "props", "i1\u{1f}a\u{1f}b"),
        Some(("items", k("i1")))
    );
    assert_eq!(elem_of_map_key(c, "props", "i1"), None, "no separator");
    assert_eq!(elem_of_map_key(c, "props", "I 1\u{1f}x"), None, "invalid");
    assert_eq!(elem_of_map_key(c, "props", "\u{1f}x"), None, "empty");
    assert_eq!(elem_of_map_key(c, "marks", "i1\u{1f}x"), None, "undeclared");
    assert_eq!(elem_of_map_key(&[], "props", "i1\u{1f}x"), None, "none");
}

/// The claim's point, and the planted fault's catch: another agent's write to `<item>\x1f<field>`
/// is refused while the item is claimed, whatever the map root the pack declares for it.
#[test]
fn a_write_to_a_claimed_elements_map_key_is_refused() {
    under_struct_mutant(|| {
        let f = fx("map");
        let a = f.person("a");
        let b = f.agent("b");
        let c =
            f.s.claim_elems(a, "items", &ks(&["i1"]), None, Some("moving".into()), false)
                .unwrap();
        let before = f.version();
        let e = f
            .respect(b, set_prop("i1", "x"))
            .expect_err("a write to a claimed element's map key must be refused");
        assert_eq!(id_of(&e), "MPC-E301");
        assert_eq!(e.report().exit(), 5);
        match &e {
            SessionError::ElemsClaimed(x) => {
                assert_eq!(x.id, c.id);
                assert_eq!(x.by, "a");
                assert_eq!(x.root, "items");
                assert_eq!(x.keys, ks(&["i1"]));
                assert_eq!(x.note.as_deref(), Some("moving"));
            }
            other => panic!("{other:?}"),
        }
        assert!(
            e.to_string().starts_with("items i1 claimed by a (a)"),
            "{e}"
        );
        assert_eq!(f.version(), before, "nothing applied");
        assert_eq!(f.map_value("props", "i1\u{1f}x"), None, "nothing applied");
        // the other composite root, and a delete of a key
        assert_eq!(
            id_of(&f.respect(b, set_in("links", "i1", "from")).unwrap_err()),
            "MPC-E301"
        );
        let del = vec![Edit::Map {
            root: "props".into(),
            ops: vec![MapOp::Delete {
                key: "i1\u{1f}x".into(),
            }],
        }];
        assert_eq!(id_of(&f.respect(b, del).unwrap_err()), "MPC-E301");
        // one claimed op refuses the whole change
        let mut both = set_prop("i2", "x");
        both.extend(set_prop("i1", "x"));
        assert_eq!(id_of(&f.respect(b, both).unwrap_err()), "MPC-E301");
        assert_eq!(f.map_value("props", "i2\u{1f}x"), None, "nothing applied");
        // another element's key passes, and an undeclared map root isn't mapped at all
        f.respect(b, set_prop("i2", "x")).unwrap();
        assert!(f.map_value("props", "i2\u{1f}x").is_some());
        f.respect(b, set_in("marks", "i1", "x")).unwrap();
        // the policy is the host's: under Ignore the same write goes through
        f.edit(b, ClaimPolicy::Ignore, set_prop("i1", "x")).unwrap();
        assert!(f.map_value("props", "i1\u{1f}x").is_some());
        assert_eq!(
            f.s.claims(a).unwrap().len(),
            1,
            "someone else's write leaves it"
        );
    });
}

/// A list op naming a claimed key is refused; an op that only anchors on one isn't.
#[test]
fn a_list_op_on_a_claimed_key_is_refused() {
    under_struct_mutant(|| {
        let f = fx("list");
        let a = f.person("a");
        let b = f.agent("b");
        f.s.claim_elems(a, "items", &ks(&["i1", "i2"]), None, None, false)
            .unwrap();
        let refused = [
            list(ListOp::Move {
                key: k("i1"),
                to: Anchor::End,
            }),
            list(ListOp::Set {
                key: k("i1"),
                value: Value::str("v"),
            }),
            delete("i2"),
        ];
        for edits in refused {
            let e = f.respect(b, edits).expect_err("a claimed key");
            assert_eq!(id_of(&e), "MPC-E301");
        }
        assert_eq!(f.live("items"), ["i1", "i2", "i3", "i4"], "nothing applied");
        // others' elements pass, placed beside the claimed ones: the claim doesn't grow
        f.respect(
            b,
            list(ListOp::Move {
                key: k("i3"),
                to: Anchor::After(k("i1")),
            }),
        )
        .unwrap();
        f.respect(
            b,
            list(ListOp::Insert {
                key: k("i5"),
                at: Anchor::After(k("i1")),
                value: Value::Null,
            }),
        )
        .unwrap();
        assert_eq!(f.live("items"), ["i1", "i5", "i3", "i2", "i4"]);
        f.respect(b, set_prop("i5", "x")).unwrap();
        // the same key in another list isn't claimed
        f.respect(
            b,
            vec![Edit::List {
                root: "groups".into(),
                ops: vec![ListOp::Delete { key: k("i1") }],
            }],
        )
        .unwrap();
        // a claimed element someone else deleted stays claimed: restoring it is a touch
        let p = f.person("p");
        f.edit(p, ClaimPolicy::Ignore, delete("i2")).unwrap();
        let restore = list(ListOp::Restore { key: k("i2") });
        assert_eq!(
            id_of(&f.respect(b, restore.clone()).unwrap_err()),
            "MPC-E301"
        );
        // under Ignore the same ops go through
        f.edit(b, ClaimPolicy::Ignore, restore).unwrap();
        assert!(f.live("items").contains(&"i2".to_owned()));
    });
}

/// Claims coordinate agents: a person is never refused, whatever the policy.
#[test]
fn a_person_is_never_refused() {
    under_struct_mutant(|| {
        let f = fx("person");
        let a = f.agent("a");
        let p = f.person("p");
        f.s.claim_elems(a, "items", &ks(&["i1"]), None, None, false)
            .unwrap();
        f.respect(p, set_prop("i1", "x")).unwrap();
        f.respect(p, delete("i2")).unwrap();
        assert_eq!(f.s.claims(a).unwrap().len(), 1);
    });
}

/// The claim ends `deleted` when none of its elements is live, and only then: by another's
/// delete, held or not, with `claim_ended` on the roster.
#[test]
fn deleting_every_claimed_element_ends_the_claim_deleted() {
    under_struct_mutant(|| {
        let f = fx("deleted");
        let a = f.agent("a");
        let p = f.person("p");
        let rx = f.s.subscribe_roster(p).unwrap();
        let c =
            f.s.claim_elems(a, "items", &ks(&["i1", "i3"]), None, None, true)
                .unwrap();
        assert_eq!((c.lines.first, c.lines.last), (1, 3));
        f.edit(p, ClaimPolicy::Ignore, delete("i1")).unwrap();
        let now = f.s.claims(a).unwrap();
        assert_eq!(now.len(), 1, "one element is still live");
        assert_eq!(f.ended(c.id), None);
        // positions are live ones: i3 is second now
        assert_eq!((now[0].lines.first, now[0].lines.last), (2, 2));
        f.edit(p, ClaimPolicy::Ignore, delete("i3")).unwrap();
        assert_eq!(f.ended(c.id), Some(ClaimEnd::Deleted));
        assert!(f.s.claims(a).unwrap().is_empty());
        f.tick();
        let evs: Vec<RosterEvent> = rx.try_iter().collect();
        assert!(
            evs.iter().any(|e| matches!(e,
                RosterEvent::ClaimStartedElems { claim, root, keys, hold: true, .. }
                    if *claim == c.id && root == "items" && *keys == ks(&["i1", "i3"]))),
            "{evs:?}"
        );
        assert!(
            evs.iter().any(|e| matches!(e,
                RosterEvent::ClaimEnded { claim, how: ClaimEnd::Deleted, .. } if *claim == c.id)),
            "{evs:?}"
        );
        assert!(
            !evs.iter()
                .any(|e| matches!(e, RosterEvent::ClaimStarted { .. })),
            "an element claim has no lines to start: {evs:?}"
        );
    });
}

/// The undo of the insert that made an element ends a claim on it too.
#[test]
fn undoing_the_insert_ends_the_claim_deleted() {
    under_struct_mutant(|| {
        let f = fx("undo");
        let p = f.person("p");
        let a = f.agent("a");
        f.edit(
            p,
            ClaimPolicy::Ignore,
            list(ListOp::Insert {
                key: k("i9"),
                at: Anchor::End,
                value: Value::Null,
            }),
        )
        .unwrap();
        let c =
            f.s.claim_elems(a, "items", &ks(&["i9"]), None, None, false)
                .unwrap();
        f.s.undo(p, 1).unwrap();
        assert!(!f.live("items").contains(&"i9".to_owned()), "undone");
        assert_eq!(f.ended(c.id), Some(ClaimEnd::Deleted));
    });
}

/// Two connections of one participant: the claim is the pid's for edits, the taking
/// connection's for leaving.
#[test]
fn two_connections_of_one_participant_share_the_claim() {
    under_struct_mutant(|| {
        let f = fx("two");
        let a1 = f.agent("a");
        let a2 = f.agent("a");
        // its own edit from the other connection isn't refused, and ends the claim
        let c =
            f.s.claim_elems(a1, "items", &ks(&["i1"]), None, None, false)
                .unwrap();
        f.respect(a2, set_prop("i2", "x")).unwrap();
        assert_eq!(f.ended(c.id), None, "another element: no touch");
        f.respect(a2, set_prop("i1", "x")).unwrap();
        assert_eq!(f.ended(c.id), Some(ClaimEnd::Edited));
        // held: it stays through its owner's edits
        let h =
            f.s.claim_elems(a1, "items", &ks(&["i1"]), None, None, true)
                .unwrap();
        f.respect(a1, set_prop("i1", "y")).unwrap();
        assert_eq!(f.ended(h.id), None);
        assert_eq!(f.s.claims(a1).unwrap().len(), 1);
        // its own overlap is no refusal
        f.s.claim_elems(a2, "items", &ks(&["i1", "i2"]), None, None, true)
            .unwrap();
        assert_eq!(f.s.claims(a1).unwrap().len(), 2);
        // the taking connection leaves: its claims end; the other's stays
        f.s.leave(a1).unwrap();
        assert_eq!(f.ended(h.id), Some(ClaimEnd::Left));
        assert_eq!(f.s.claims(a2).unwrap().len(), 1);
    });
}

/// A drag: with `hold` the claim survives its owner's several edits and ends on release; without
/// it, the first edit touching it ends it `edited`, and the roster says so.
#[test]
fn hold_survives_its_owners_edits_and_no_hold_ends_edited() {
    under_struct_mutant(|| {
        let f = fx("hold");
        let a = f.agent("a");
        let p = f.person("p");
        let rx = f.s.subscribe_roster(p).unwrap();
        let held =
            f.s.claim_elems(a, "items", &ks(&["i1"]), None, None, true)
                .unwrap();
        for field in ["x", "y", "w", "h"] {
            f.respect(a, set_prop("i1", field)).unwrap();
            assert_eq!(f.ended(held.id), None, "held through the write to {field}");
        }
        f.respect(
            a,
            list(ListOp::Move {
                key: k("i1"),
                to: Anchor::End,
            }),
        )
        .unwrap();
        assert_eq!(f.ended(held.id), None, "held through a move");
        assert_eq!(f.s.claims(a).unwrap().len(), 1);
        assert_eq!(f.s.release_claim(a, None).unwrap(), [held.id]);
        assert_eq!(f.ended(held.id), Some(ClaimEnd::Released));

        let once =
            f.s.claim_elems(a, "items", &ks(&["i2"]), None, None, false)
                .unwrap();
        // an edit elsewhere doesn't touch it
        f.respect(a, set_prop("i3", "x")).unwrap();
        assert_eq!(f.ended(once.id), None);
        f.respect(a, set_prop("i2", "x")).unwrap();
        assert_eq!(f.ended(once.id), Some(ClaimEnd::Edited));
        assert!(f.s.claims(a).unwrap().is_empty());
        f.tick();
        let evs: Vec<RosterEvent> = rx.try_iter().collect();
        let how = |id: ClaimId| {
            evs.iter().find_map(|e| match e {
                RosterEvent::ClaimEnded { claim, how, .. } if *claim == id => Some(*how),
                _ => None,
            })
        };
        assert_eq!(how(held.id), Some(ClaimEnd::Released), "{evs:?}");
        assert_eq!(how(once.id), Some(ClaimEnd::Edited), "{evs:?}");
    });
}

/// What comes off the wire is bounded: a note by [`CLAIM_NOTE_MAX`], and a held claim by its
/// time, which any claim's maximum clamps, so `hold` can't keep elements for ever.
#[test]
fn a_claims_note_and_time_are_bounded() {
    under_struct_mutant(|| {
        let f = fx("bounds");
        let a = f.agent("a");
        let b = f.agent("b");
        let claim = |note: String| {
            f.s.claim_elems(a, "items", &ks(&["i1"]), None, Some(note), true)
        };
        let e = claim("n".repeat(CLAIM_NOTE_MAX + 1)).unwrap_err();
        assert_eq!(id_of(&e), "MPC-E304");
        assert!(f.s.claims(a).unwrap().is_empty(), "nothing was taken");
        let most = claim("n".repeat(CLAIM_NOTE_MAX)).unwrap();
        f.s.release_claim(a, Some(most.id)).unwrap();

        let max = Config::default().claim_max_ms;
        let held =
            f.s.claim_elems(a, "items", &ks(&["i2"]), Some(u64::MAX), None, true)
                .unwrap();
        assert_eq!(held.remaining_ms, max, "clamped to the most");
        // its time runs out, held or not, and another agent gets through
        assert!(f.respect(b, set_prop("i2", "x")).is_err());
        f.host.now.fetch_add(max, Ordering::SeqCst);
        f.respect(b, set_prop("i2", "x")).unwrap();
        assert_eq!(f.ended(held.id), Some(ClaimEnd::Expired));
    });
}

/// One claim on `i1` held by an agent, and a person whose later change on another element is what
/// would end a mark an earlier change left behind.
fn claim_then_later(name: &str) -> (Fx, ConnId, ConnId, ClaimId) {
    let f = fx(name);
    let a = f.agent("a");
    let p = f.person("p");
    let c =
        f.s.claim_elems(a, "items", &ks(&["i1"]), None, None, false)
            .unwrap();
    (f, a, p, c.id)
}

#[track_caller]
fn later_change_leaves_it(f: &Fx, p: ConnId, c: ClaimId, field: &str) {
    f.edit(p, ClaimPolicy::Ignore, set_prop("i3", field))
        .unwrap();
    assert_eq!(
        f.ended(c),
        None,
        "someone else's later change ended the claim"
    );
}

/// The apply's `Ok(None)` exit: an owner's change that changes nothing leaves no mark, whether it
/// is a list op or a map op. Its own later op that lands still ends the claim.
#[test]
fn a_change_that_does_not_land_leaves_the_claim_alone() {
    under_struct_mutant(|| {
        let (f, a, p, c) = claim_then_later("noland");
        // a restore of a live element, a delete of an absent key
        let nothing = [
            list(ListOp::Restore { key: k("i1") }),
            vec![Edit::Map {
                root: "props".into(),
                ops: vec![MapOp::Delete {
                    key: "i1\u{1f}x".into(),
                }],
            }],
        ];
        for (n, edits) in nothing.into_iter().enumerate() {
            let v = f.version();
            f.respect(a, edits).unwrap();
            assert_eq!(f.version(), v, "it changed nothing");
            assert_eq!(f.ended(c), None, "its own change that did nothing");
            later_change_leaves_it(&f, p, c, &format!("n{n}"));
        }
        assert_eq!(f.s.claims(a).unwrap().len(), 1);
        f.respect(
            a,
            list(ListOp::Set {
                key: k("i1"),
                value: Value::str("v"),
            }),
        )
        .unwrap();
        assert_eq!(f.ended(c), Some(ClaimEnd::Edited), "its own op that lands");
    });
}

/// The apply's `Err` exit: the engine refuses an owner's change, which leaves no mark. The refusal
/// is pinned as the engine's own, so this can't drift to one the session makes before the commit
/// (a drop through `edit_ops` is one: it never reaches the apply).
#[test]
fn a_refused_change_leaves_the_claim_alone() {
    under_struct_mutant(|| {
        let (f, a, p, c) = claim_then_later("refused");
        let v = f.version();
        let e = f
            .respect(
                a,
                list(ListOp::Move {
                    key: k("i1"),
                    to: Anchor::After(k("i1")),
                }),
            )
            .expect_err("a move next to itself is refused");
        assert!(
            matches!(
                &e,
                SessionError::Doc(DocError::Op {
                    refusal: OpRefusal::SelfAnchor(_),
                    ..
                })
            ),
            "not the engine's refusal: {e:?}"
        );
        assert_eq!(f.version(), v, "nothing applied");
        assert_eq!(f.ended(c), None, "a reload would have ended it: {e:?}");
        later_change_leaves_it(&f, p, c, "refused");
        assert_eq!(f.s.claims(a).unwrap().len(), 1);
    });
}

/// The exit after the doc took the change: its bookkeeping fails, so the session reloads, and the
/// change never reaches the store. The claim ends `reset`, not `edited`, and nothing waits for the
/// next change.
#[test]
fn a_change_whose_bookkeeping_fails_resets_the_claim() {
    use k2_mpc_base::differential::{with_apply_fault, ApplyFault};
    under_struct_mutant(|| {
        let (f, a, p, c) = claim_then_later("bookkeeping");
        let v = f.version();
        let e = with_apply_fault(ApplyFault::BookkeepingFailsAfter(0), || {
            f.respect(a, set_prop("i1", "x"))
        })
        .expect_err("the planted failure");
        assert!(e.to_string().contains("a planted failure"), "{e:?}");
        assert_eq!(f.version(), v, "it never reached the store");
        assert_eq!(f.map_value("props", "i1\u{1f}x"), None, "the doc reloaded");
        assert_eq!(f.ended(c), Some(ClaimEnd::Reset), "the reload ends it");
        assert!(f.s.claims(p).unwrap().is_empty());
        // and the doc goes on: a later change lands
        f.edit(p, ClaimPolicy::Ignore, set_prop("i3", "x")).unwrap();
        assert!(f.version() > v);
    });
}

/// A change that lands with an op on the claimed element that has no effect: that op touches
/// nothing, so the claim stays, and no mark waits for the next change.
#[test]
fn an_op_with_no_effect_beside_one_that_lands_leaves_the_claim_alone() {
    under_struct_mutant(|| {
        let (f, a, p, c) = claim_then_later("mixed");
        let v = f.version();
        let mixed = vec![Edit::List {
            root: "items".into(),
            ops: vec![
                ListOp::Restore { key: k("i1") },
                ListOp::Set {
                    key: k("i2"),
                    value: Value::str("v"),
                },
            ],
        }];
        f.respect(a, mixed).unwrap();
        assert!(f.version() > v, "it landed");
        assert_eq!(f.ended(c), None, "an op with no effect touched nothing");
        later_change_leaves_it(&f, p, c, "mixed");
    });
}

/// The same for a hand-off: the waiter's change that doesn't land doesn't use its reservation.
#[test]
fn a_change_that_does_not_land_leaves_the_reservation_alone() {
    under_struct_mutant(|| {
        let f = fx("noland-res");
        let a = f.agent("a");
        let b = f.agent("b");
        let c = f.agent("c");
        let p = f.person("p");
        let claim =
            f.s.claim_elems(a, "items", &ks(&["i1"]), None, None, false)
                .unwrap();
        f.s.register_waiter(b, claim.id).unwrap();
        f.s.register_waiter(c, claim.id).unwrap();
        f.s.release_claim(a, Some(claim.id)).unwrap();
        let holder = || f.s.reservations(p).unwrap()[0].pid.clone();
        assert_eq!(holder(), "b");
        f.respect(b, list(ListOp::Restore { key: k("i1") }))
            .unwrap();
        assert_eq!(holder(), "b", "its own change that did nothing");
        f.edit(p, ClaimPolicy::Ignore, set_prop("i3", "x")).unwrap();
        assert_eq!(
            holder(),
            "b",
            "someone else's later change used the reservation"
        );
        // its own op that lands uses it, and it passes on
        f.respect(
            b,
            list(ListOp::Set {
                key: k("i1"),
                value: Value::str("v"),
            }),
        )
        .unwrap();
        assert_eq!(holder(), "c");
    });
}

/// [`Pack::composite`]'s stated limit: a key of a composite root that maps to no element isn't
/// covered by any claim, so the write goes through under `Respect`.
#[test]
fn a_map_key_that_maps_to_no_element_is_covered_by_no_claim() {
    under_struct_mutant(|| {
        let f = fx("unmapped");
        let a = f.person("a");
        let b = f.agent("b");
        f.s.claim_elems(a, "items", &ks(&["i1"]), None, None, true)
            .unwrap();
        // no separator, and a start that isn't a valid key
        for key in ["i1", "I 1\u{1f}x"] {
            let write = vec![Edit::Map {
                root: "props".into(),
                ops: vec![MapOp::Set {
                    key: key.into(),
                    value: Value::str("v"),
                }],
            }];
            f.respect(b, write).unwrap();
            assert!(f.map_value("props", key).is_some(), "{key:?}");
        }
    });
}

/// A claim belongs to the connection that took it, `hold` or not: it ends `left` when that
/// connection closes. A new connection that names the old one closes it at once; one that doesn't
/// leaves the claim live until the old one closes or its time runs out.
#[test]
fn a_held_claim_ends_with_its_connection() {
    under_struct_mutant(|| {
        let f = fx("conn");
        let join = |prev: Option<u64>| {
            let p = Participant::new("a", "a", "#000000", ParticipantKind::Agent, "app");
            match prev {
                Some(c) => f.s.join_replacing(p, Stay::Held, c).unwrap(),
                None => f.s.join(p, Stay::Held).unwrap(),
            }
        };
        let first = join(None);
        let held =
            f.s.claim_elems(first.conn, "items", &ks(&["i1"]), None, None, true)
                .unwrap();
        // a second connection that doesn't name the first: the claim stays
        let second = join(None);
        assert_eq!(f.ended(held.id), None);
        assert_eq!(f.s.claims(second.conn).unwrap().len(), 1);
        // one that names it closes it, and the claim ends with it, held or not
        let third = join(Some(first.client));
        assert_eq!(f.ended(held.id), Some(ClaimEnd::Left));
        assert!(f.s.claims(third.conn).unwrap().is_empty());
    });
}

/// A claim is usually taken before a first edit, when a person's client ID has no owner row yet.
/// It names its holder from the live connection, not the store: the refusal, the roster's start
/// and a later subscriber's snapshot all say who. A person's edit is never refused, so the one
/// refused here is an agent's.
#[test]
fn a_person_who_has_not_written_yet_is_named_by_their_claim() {
    under_struct_mutant(|| {
        let f = fx("unwritten");
        let key = DocKey {
            space: "ws".into(),
            path: "a.items".into(),
        };
        let id = f.store.find(&key).unwrap().expect("the doc's row");
        let ada =
            f.s.join(
                Participant::new("ada", "Ada", "#000000", ParticipantKind::Person, "app"),
                Stay::Held,
            )
            .unwrap();
        let owner = || f.store.client_owner(id, ada.client).unwrap();
        assert_eq!(owner(), None, "no owner row before a first change");
        let bot = f.agent("bot");
        let p = f.person("p");
        let rx = f.s.subscribe_roster(p).unwrap();
        let c =
            f.s.claim_elems(ada.conn, "items", &ks(&["i1"]), None, None, false)
                .unwrap();
        assert_eq!((c.pid.as_str(), c.name.as_str()), ("ada", "Ada"));
        assert_eq!(owner(), None, "a claim writes no owner row");

        let e = f.respect(bot, set_prop("i1", "x")).unwrap_err();
        match &e {
            SessionError::ElemsClaimed(x) => {
                assert_eq!((x.by.as_str(), x.name.as_str()), ("ada", "Ada"));
                assert_eq!(x.id, c.id);
            }
            other => panic!("{other:?}"),
        }
        assert!(e.to_string().contains("claimed by Ada (ada)"), "{e}");

        f.tick();
        let evs: Vec<RosterEvent> = rx.try_iter().collect();
        assert!(
            evs.iter().any(|e| matches!(e,
                RosterEvent::ClaimStartedElems { who, claim, .. }
                    if *who == Who::Pid("ada".into()) && *claim == c.id)),
            "{evs:?}"
        );
        let late = f.s.subscribe_roster(p).unwrap().try_recv().unwrap();
        let RosterEvent::Snapshot { members, .. } = late else {
            panic!("{late:?}");
        };
        let m = members
            .iter()
            .find(|m| m.who == Who::Pid("ada".into()))
            .expect("the holder");
        assert_eq!(m.entry.claims[0].id, c.id);
        assert_eq!(owner(), None, "still no owner row");

        // the first change lands the row, and ends the claim
        f.edit(ada.conn, ClaimPolicy::Ignore, set_prop("i1", "x"))
            .unwrap();
        assert_eq!(owner().as_deref(), Some("ada"));
        assert_eq!(f.ended(c.id), Some(ClaimEnd::Edited));
    });
}

/// Under `Respect` a line claim and an element claim on one doc each refuse what touches them, by
/// their own refusal, and neither refuses what touches only the other's root. A line claim can't
/// be taken on a list root.
#[test]
fn line_claims_and_element_claims_refuse_only_what_touches_them() {
    under_struct_mutant(|| {
        let f = fx("cross");
        let a = f.agent("a");
        let b = f.agent("b");
        let text = |start, end, insert: &str| {
            vec![Edit::Text {
                root: "notes".into(),
                hunks: vec![Hunk {
                    start,
                    end,
                    insert: insert.into(),
                }],
            }]
        };
        let line_one = k2_mpc_base::session::LineRange {
            root: "notes".into(),
            first: 1,
            last: 1,
        };
        // both hold, at the same line numbers: they cover different roots
        let elems =
            f.s.claim_elems(a, "items", &ks(&["i1"]), None, None, false)
                .unwrap();
        assert_eq!((elems.lines.first, elems.lines.last), (1, 1));
        let lines = f.s.claim(b, line_one, None, None, false).unwrap();
        assert_eq!(f.s.claims(a).unwrap().len(), 2);

        // the element claim refuses b's write to its element, by its own refusal
        let e = f.respect(b, set_prop("i1", "x")).unwrap_err();
        assert!(
            matches!(&e, SessionError::ElemsClaimed(x) if x.id == elems.id && x.by == "a"),
            "{e:?}"
        );
        // the line claim refuses a's edit of its line, by its own, in lines
        let e = f.respect(a, text(0, 5, "FIRST")).unwrap_err();
        assert!(
            matches!(&e, SessionError::Claimed { id, by, lines: (1, 1), .. }
                if *id == lines.id && by == "b"),
            "{e:?}"
        );
        // what touches only the other root, or another place, goes through and ends nothing
        f.respect(a, text(6, 12, "SECOND")).unwrap();
        f.respect(b, set_prop("i2", "x")).unwrap();
        assert_eq!(f.ended(elems.id), None);
        assert_eq!(f.ended(lines.id), None);

        // each owner's own edit ends only its own claim
        f.respect(b, text(0, 5, "FIRST")).unwrap();
        assert_eq!(f.ended(lines.id), Some(ClaimEnd::Edited));
        assert_eq!(f.ended(elems.id), None);
        f.respect(a, set_prop("i1", "x")).unwrap();
        assert_eq!(f.ended(elems.id), Some(ClaimEnd::Edited));

        // a list root has no lines to claim: refused, and nothing taken
        let on_list = k2_mpc_base::session::LineRange {
            root: "items".into(),
            first: 1,
            last: 1,
        };
        let e = f.s.claim(a, on_list, None, None, false).unwrap_err();
        assert!(
            matches!(&e, SessionError::Doc(DocError::NoRoot { .. })),
            "{e:?}"
        );
        assert!(f.s.claims(a).unwrap().is_empty());
    });
}

/// Two claims on a root overlap when their key sets meet; other lists and other keys are free.
#[test]
fn claims_overlap_when_their_keys_meet() {
    under_struct_mutant(|| {
        let f = fx("overlap");
        let a = f.agent("a");
        let b = f.agent("b");
        let c =
            f.s.claim_elems(a, "items", &ks(&["i2", "i3"]), Some(5_000), None, false)
                .unwrap();
        assert_eq!(
            c.span,
            ClaimSpan::Elems {
                root: "items".into(),
                keys: ks(&["i2", "i3"])
            }
        );
        assert_eq!((c.start, c.end), (0, 0));
        let e =
            f.s.claim_elems(b, "items", &ks(&["i1", "i3"]), None, None, false)
                .unwrap_err();
        assert_eq!(id_of(&e), "MPC-E301");
        assert!(
            matches!(&e, SessionError::ElemsClaimed(x) if x.id == c.id && x.remaining_ms == 5_000),
            "{e:?}"
        );
        // no change to the first claim, and none taken
        assert_eq!(f.s.claims(a).unwrap().len(), 1);
        f.s.claim_elems(b, "items", &ks(&["i1", "i4"]), None, None, false)
            .unwrap();
        f.s.claim_elems(b, "groups", &ks(&["i1"]), None, None, false)
            .unwrap();
        // duplicates collapse; the span is in key order
        let d =
            f.s.claim_elems(a, "groups", &ks(&["g2", "g2"]), None, None, false)
                .unwrap();
        assert_eq!(
            d.span,
            ClaimSpan::Elems {
                root: "groups".into(),
                keys: ks(&["g2"])
            }
        );
        // the other way in
        let via =
            f.s.claim_span(
                a,
                ClaimSpan::Elems {
                    root: "items".into(),
                    keys: ks(&["i4"]),
                },
                None,
                None,
                false,
            )
            .unwrap_err();
        assert_eq!(id_of(&via), "MPC-E301");
    });
}

#[test]
fn a_claim_span_renders_its_keys() {
    let span = |keys: &[&str]| ClaimSpan::Elems {
        root: "items".into(),
        keys: ks(keys),
    };
    assert_eq!(span(&["i1"]).to_string(), "items i1");
    assert_eq!(span(&["i1", "i2"]).to_string(), "items i1, i2");
    assert_eq!(
        span(&["i1", "i2", "i3", "i4", "i5"]).to_string(),
        "items i1, i2 (+3)"
    );
}

#[test]
fn bad_claims_are_refused_with_their_ids() {
    under_struct_mutant(|| {
        let f = fx("bad");
        let a = f.agent("a");
        let claim = |root: &str, keys: &[Key]| {
            f.s.claim_elems(a, root, keys, None, None, false)
                .unwrap_err()
        };
        // E304: no keys, too many, no time
        assert_eq!(id_of(&claim("items", &[])), "MPC-E304");
        let many: Vec<Key> = (0..=CLAIM_KEYS_MAX).map(|n| k(&format!("k{n}"))).collect();
        assert_eq!(many.len(), 257);
        assert_eq!(id_of(&claim("items", &many)), "MPC-E304");
        // E114, and the message says why: a root that isn't a list (a map, a text, none)
        for root in ["props", "notes", "nowhere"] {
            let e = claim(root, &ks(&["i1"]));
            assert_eq!(id_of(&e), "MPC-E114", "{root}");
            assert!(e.to_string().contains("is not a list root"), "{root}: {e}");
        }
        let zero =
            f.s.claim_elems(a, "items", &ks(&["i1"]), Some(0), None, false)
                .unwrap_err();
        assert_eq!(id_of(&zero), "MPC-E304");
        // E114: unknown or deleted, the whole claim
        assert_eq!(id_of(&claim("items", &ks(&["i1", "nope"]))), "MPC-E114");
        f.edit(a, ClaimPolicy::Ignore, delete("i4")).unwrap();
        assert_eq!(id_of(&claim("items", &ks(&["i4"]))), "MPC-E114");
        assert!(f.s.claims(a).unwrap().is_empty(), "nothing was taken");
        // it needs edit
        let v = f.person("viewer");
        let e =
            f.s.claim_elems(v, "items", &ks(&["i1"]), None, None, false)
                .unwrap_err();
        assert!(matches!(e, SessionError::Forbidden { .. }), "{e:?}");
        // exactly the most is allowed
        let mut most: Vec<Key> = Vec::new();
        for n in 0..CLAIM_KEYS_MAX {
            most.push(k(&format!("n{n}")));
        }
        let ops = most
            .iter()
            .map(|key| ListOp::Insert {
                key: key.clone(),
                at: Anchor::End,
                value: Value::Null,
            })
            .collect();
        f.edit(a, ClaimPolicy::Ignore, list_of(ops)).unwrap();
        let c =
            f.s.claim_elems(a, "items", &most, None, None, false)
                .unwrap();
        assert!(c.span.to_string().ends_with("(+254)"), "{}", c.span);
    });
}

fn list_of(ops: Vec<ListOp>) -> Vec<Edit> {
    vec![Edit::List {
        root: "items".into(),
        ops,
    }]
}

/// A table's claims stay rows, columns or cells: no element claim, by either way in.
#[test]
fn a_table_refuses_element_claims() {
    let dir = std::env::temp_dir().join(format!("k2mpc-elem-table-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("d.redb")).unwrap());
    let host = Arc::new(H {
        now: AtomicU64::new(1),
        events: Mutex::new(Vec::new()),
    });
    let mgr = SessionManager::new(store, host, Config::default());
    let key = DocKey {
        space: "ws".into(),
        path: "a.csv".into(),
    };
    let s = mgr
        .open(Arc::new(TablePack), &key, Some(b"a,b\n1,2\n3,4\n"))
        .unwrap();
    let p = Participant::new("a", "a", "#000000", ParticipantKind::Agent, "app");
    let c = s.join(p, Stay::Held).unwrap().conn;
    let rows = s.keyed_state_for_tests().unwrap();
    let row = match rows
        .roots
        .iter()
        .find(|(r, _)| r == k2_mpc_base::table::ROWS)
    {
        Some((_, KeyedRoot::List { live, .. })) => live[0].0.clone(),
        other => panic!("{other:?}"),
    };
    let e = s
        .claim_elems(
            c,
            k2_mpc_base::table::ROWS,
            std::slice::from_ref(&row),
            None,
            None,
            false,
        )
        .unwrap_err();
    assert_eq!(id_of(&e), "MPC-E304");
    let e = s
        .claim_span(
            c,
            ClaimSpan::Elems {
                root: k2_mpc_base::table::ROWS.into(),
                keys: vec![row.clone()],
            },
            None,
            None,
            false,
        )
        .unwrap_err();
    assert_eq!(id_of(&e), "MPC-E304");
    // the table's own claim still works
    s.claim_span(
        c,
        ClaimSpan::Rows {
            first: row.clone(),
            last: row,
        },
        None,
        None,
        false,
    )
    .unwrap();
    drop(s);
    drop(mgr);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_caps_hold_with_their_ids() {
    under_struct_mutant(|| {
        let mut config = Config::default();
        config.claims_per_participant = 2;
        config.claims_max = 3;
        config.claim_waiters_max = 1;
        let f = fx_with("caps", config);
        let a = f.agent("a");
        let b = f.agent("b");
        let c = f.agent("c");
        let claim = |conn, key: &str| {
            f.s.claim_elems(conn, "items", &ks(&[key]), None, None, false)
        };
        let first = claim(a, "i1").unwrap();
        claim(a, "i2").unwrap();
        assert_eq!(id_of(&claim(a, "i3").unwrap_err()), "MPC-E904");
        claim(b, "i3").unwrap();
        assert_eq!(id_of(&claim(b, "i4").unwrap_err()), "MPC-E903");
        f.s.register_waiter(b, first.id).unwrap();
        assert_eq!(
            id_of(&f.s.register_waiter(c, first.id).unwrap_err()),
            "MPC-E905"
        );
    });
}

/// The queue, as for lines: waiters in order, no renewing or re-claiming past them, the elements
/// held for the first waiter when the claim ends, binding other agents, then passed on.
#[test]
fn waiters_get_the_elements_in_order() {
    under_struct_mutant(|| {
        let f = fx("waiters");
        let a = f.agent("a");
        let b = f.agent("b");
        let c = f.agent("c");
        let d = f.agent("d");
        let claim =
            f.s.claim_elems(a, "items", &ks(&["i1", "i2"]), None, None, false)
                .unwrap();
        assert_eq!(f.s.register_waiter(b, claim.id).unwrap().position, 1);
        let w = f.s.register_waiter(c, claim.id).unwrap();
        assert_eq!(w.position, 2);
        assert_eq!(w.claim.waiters, ["b", "c"]);
        // anti-starvation
        let e =
            f.s.renew_claim(a, Some(claim.id), Some(3_600_000), None)
                .unwrap_err();
        assert_eq!(id_of(&e), "MPC-E303");
        let e =
            f.s.claim_elems(a, "items", &ks(&["i2"]), None, None, false)
                .unwrap_err();
        assert_eq!(id_of(&e), "MPC-E303");

        f.s.release_claim(a, Some(claim.id)).unwrap();
        assert_eq!(f.ended(claim.id), Some(ClaimEnd::Released));
        let r = f.s.reservations(a).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].pid, "b");
        assert_eq!(
            r[0].span,
            ClaimSpan::Elems {
                root: "items".into(),
                keys: ks(&["i1", "i2"])
            }
        );
        // held for b: another agent can't edit there or claim it; a person isn't bound
        let e = f.respect(d, set_prop("i1", "x")).unwrap_err();
        assert!(
            matches!(&e, SessionError::ElemsClaimed(x) if x.id == claim.id && x.by == "b"),
            "{e:?}"
        );
        let e =
            f.s.claim_elems(d, "items", &ks(&["i2"]), None, None, false)
                .unwrap_err();
        assert_eq!(id_of(&e), "MPC-E301");
        // b's own edit there uses it: it passes to c, the next in line
        f.respect(b, set_prop("i1", "x")).unwrap();
        let r = f.s.reservations(a).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].pid, "c");
        // c claims: its reservation is used, and it holds the elements
        let got =
            f.s.claim_elems(c, "items", &ks(&["i1"]), None, None, false)
                .unwrap();
        assert!(f.s.reservations(a).unwrap().is_empty());
        assert_eq!(got.pid, "c");
    });
}

/// A reservation with nothing live ends `deleted`: nothing passes on.
#[test]
fn a_reservation_on_deleted_elements_ends_and_passes_nothing() {
    under_struct_mutant(|| {
        let f = fx("res-del");
        let a = f.agent("a");
        let b = f.agent("b");
        let c = f.agent("c");
        let p = f.person("p");
        let claim =
            f.s.claim_elems(a, "items", &ks(&["i1"]), None, None, false)
                .unwrap();
        f.s.register_waiter(b, claim.id).unwrap();
        f.s.register_waiter(c, claim.id).unwrap();
        f.s.release_claim(a, Some(claim.id)).unwrap();
        assert_eq!(f.s.reservations(p).unwrap().len(), 1);
        f.edit(p, ClaimPolicy::Ignore, delete("i1")).unwrap();
        assert!(f.s.reservations(p).unwrap().is_empty());
        let how = f.host.events.lock().unwrap().iter().find_map(|e| match e {
            AuditEvent::ReservationEnded { how, .. } => Some(*how),
            _ => None,
        });
        assert_eq!(how, Some(ReservationEnd::Deleted));
    });
}

/// Time runs out on the host's clock: the claim ends and its elements are free.
#[test]
fn a_claim_expires_and_frees_its_elements() {
    under_struct_mutant(|| {
        let f = fx("expire");
        let a = f.agent("a");
        let b = f.agent("b");
        let c =
            f.s.claim_elems(a, "items", &ks(&["i1"]), Some(1_000), None, false)
                .unwrap();
        assert_eq!(c.remaining_ms, 1_000);
        assert!(f.respect(b, set_prop("i1", "x")).is_err());
        f.host.now.fetch_add(1_000, Ordering::SeqCst);
        f.respect(b, set_prop("i1", "x")).unwrap();
        assert_eq!(f.ended(c.id), Some(ClaimEnd::Expired));
        // renewing and the time left read the element claim's info
        let again =
            f.s.claim_elems(a, "items", &ks(&["i2"]), Some(1_000), None, false)
                .unwrap();
        assert_eq!(f.s.claim_remaining_ms(a, again.id).unwrap(), 1_000);
        let renewed =
            f.s.renew_claim(a, Some(again.id), Some(2_000), None)
                .unwrap();
        assert_eq!(renewed.remaining_ms, 2_000);
        assert_eq!(renewed.span, again.span);
    });
}

/// Someone who subscribes after a claim was taken sees who holds what: the first snapshot's
/// entries name it, as does the roster call.
#[test]
fn a_late_subscriber_sees_element_claims_in_its_snapshot() {
    under_struct_mutant(|| {
        let f = fx("late");
        let a = f.agent("a");
        let p = f.person("p");
        let c =
            f.s.claim_elems(a, "items", &ks(&["i2", "i1"]), None, Some("n".into()), true)
                .unwrap();
        f.tick();
        let rx = f.s.subscribe_roster(p).unwrap();
        let first = rx.try_recv().expect("a snapshot first");
        let RosterEvent::Snapshot { members, .. } = first else {
            panic!("{first:?}");
        };
        let m = members
            .iter()
            .find(|m| m.who == Who::Pid("a".into()))
            .expect("the claimant");
        assert_eq!(m.entry.claims.len(), 1);
        let got = &m.entry.claims[0];
        assert_eq!(got.id, c.id);
        assert_eq!(
            got.span,
            ClaimSpan::Elems {
                root: "items".into(),
                keys: ks(&["i1", "i2"])
            }
        );
        assert_eq!(got.note.as_deref(), Some("n"));
        assert!(got.hold);
        let rows = f.s.roster(p).unwrap();
        let row = rows.iter().find(|r| r.participant.pid == "a").unwrap();
        assert_eq!(row.claims.len(), 1);
    });
}

/// An App viewer sees other participants by handle: in a refusal, a claim and a roster event.
#[test]
fn an_app_viewer_never_sees_a_pid() {
    under_struct_mutant(|| {
        let f = fx("app");
        let a = f.agent("secret-pid");
        let app = f.join("app-1", ParticipantKind::App);
        let rx = f.s.subscribe_roster(app).unwrap();
        let held =
            f.s.claim_elems(a, "items", &ks(&["i1"]), None, None, false)
                .unwrap();
        let e =
            f.s.claim_elems(app, "items", &ks(&["i1"]), None, None, false)
                .unwrap_err();
        let SessionError::ElemsClaimed(x) = &e else {
            panic!("{e:?}");
        };
        assert_ne!(x.by, "secret-pid");
        let seen = f.s.claims(app).unwrap();
        assert_eq!(seen[0].id, held.id);
        assert_ne!(seen[0].pid, "secret-pid");
        f.tick();
        let evs: Vec<RosterEvent> = rx.try_iter().collect();
        let who = evs
            .iter()
            .find_map(|e| match e {
                RosterEvent::ClaimStartedElems { who, .. } => Some(who.clone()),
                _ => None,
            })
            .expect("the claim's start");
        assert!(who.pid().is_none(), "{who:?}");
    });
}

/// A doc with a text root too: line claims and element claims live side by side, and a text
/// edit neither ends nor moves an element claim.
#[test]
fn text_claims_and_element_claims_live_side_by_side() {
    under_struct_mutant(|| {
        let f = fx("mixed");
        let a = f.agent("a");
        let b = f.agent("b");
        let held =
            f.s.claim_elems(a, "items", &ks(&["i1"]), None, None, true)
                .unwrap();
        let lines =
            f.s.claim(
                b,
                k2_mpc_base::session::LineRange {
                    root: "notes".into(),
                    first: 1,
                    last: 1,
                },
                None,
                None,
                false,
            )
            .unwrap();
        let edit = vec![Edit::Text {
            root: "notes".into(),
            hunks: vec![Hunk {
                start: 0,
                end: 0,
                insert: "top\n".into(),
            }],
        }];
        f.edit(a, ClaimPolicy::Ignore, edit).unwrap();
        let now = f.s.claims(a).unwrap();
        assert_eq!(now.len(), 2, "{now:?}");
        let mine = now.iter().find(|c| c.id == held.id).unwrap();
        assert_eq!(mine.span, held.span);
        assert_eq!(mine.lines, held.lines);
        assert!(now.iter().any(|c| c.id == lines.id));
        // the release that ends it reads no text root
        assert_eq!(f.s.release_claim(a, Some(held.id)).unwrap(), [held.id]);
        assert_eq!(f.ended(held.id), Some(ClaimEnd::Released));
    });
}
